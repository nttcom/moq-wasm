use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::ValueEnum;
use mediapack::Timestamp;
use moqt::TrackObject;

use crate::{
    file_writer::RecordingFile,
    pipeline::{FileTracks, Output, Pipeline, capture_timestamp},
    track_selection::Selection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Layout {
    /// The video and the audio in one file.
    Combined,
    /// Each track in its own file, `<stem>-<track>.mp4`.
    PerTrack,
}

/// About 10 s of 20 ms frames, well past the one group the audio can lead
/// the video by. Beyond it the oldest held object is dropped: it is the most
/// likely to precede the video.
const MAX_PENDING_AUDIO: usize = 500;

/// The files of one recording. They share one capture origin, the capture
/// timestamp of the first video object, so their timelines line up. Each
/// track subscribes from its own next group, so the audio can start a group
/// before the video, and an audio object can overtake the video's first: the
/// audio waits for the origin, and audio captured before it is dropped.
pub(crate) struct Recording {
    /// The video's file is at index 0.
    files: Vec<TrackFile>,
    audio: Option<usize>,
    capture_origin: Option<Timestamp>,
    pending_audio: VecDeque<TrackObject>,
}

struct TrackFile {
    pipeline: Pipeline,
    file: RecordingFile,
}

impl Recording {
    pub(crate) async fn create(
        selection: &Selection,
        layout: Layout,
        path: PathBuf,
        segment_length: Option<Duration>,
    ) -> Result<Self> {
        let video = Some(selection.video.codec);
        let audio = selection.audio.as_ref().map(|audio| audio.config.clone());
        let mut files = Vec::new();
        let audio_index = match layout {
            Layout::Combined => {
                let has_audio = audio.is_some();
                files.push(
                    TrackFile::create(FileTracks { video, audio }, path, segment_length).await?,
                );
                has_audio.then_some(0)
            }
            Layout::PerTrack => {
                let video_path = track_path(&path, &selection.video.name);
                let video_tracks = FileTracks { video, audio: None };
                files.push(TrackFile::create(video_tracks, video_path, segment_length).await?);
                match &selection.audio {
                    Some(track) => {
                        let audio_tracks = FileTracks { video: None, audio };
                        let audio_path = track_path(&path, &track.name);
                        files.push(
                            TrackFile::create(audio_tracks, audio_path, segment_length).await?,
                        );
                        Some(1)
                    }
                    None => None,
                }
            }
        };
        Ok(Self {
            files,
            audio: audio_index,
            capture_origin: None,
            pending_audio: VecDeque::new(),
        })
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|file| file.file.path())
    }

    pub(crate) async fn push_video(&mut self, object: &TrackObject) -> Result<()> {
        if self.capture_origin.is_none() {
            self.capture_origin = capture_timestamp(object);
        }
        let file = &mut self.files[0];
        let output = file.pipeline.push_video(object, self.capture_origin)?;
        file.write(output).await?;
        let Some(origin) = self.capture_origin else {
            return Ok(());
        };
        for audio in std::mem::take(&mut self.pending_audio) {
            self.write_audio(&audio, origin).await?;
        }
        Ok(())
    }

    pub(crate) async fn push_audio(&mut self, object: &TrackObject) -> Result<()> {
        match self.capture_origin {
            Some(origin) => self.write_audio(object, origin).await,
            None => {
                if self.pending_audio.len() == MAX_PENDING_AUDIO {
                    self.pending_audio.pop_front();
                }
                self.pending_audio.push_back(object.clone());
                Ok(())
            }
        }
    }

    async fn write_audio(&mut self, object: &TrackObject, origin: Timestamp) -> Result<()> {
        if capture_timestamp(object).is_some_and(|capture| capture < origin) {
            return Ok(());
        }
        let index = self.audio.context("audio object without an audio track")?;
        let file = &mut self.files[index];
        let output = file.pipeline.push_audio(object, Some(origin))?;
        file.write(output).await
    }

    pub(crate) async fn finish(self) -> Result<()> {
        for mut file in self.files {
            let tail = file.pipeline.finish()?;
            file.file.finish(&tail).await?;
        }
        Ok(())
    }
}

impl TrackFile {
    async fn create(
        tracks: FileTracks,
        path: PathBuf,
        segment_length: Option<Duration>,
    ) -> Result<Self> {
        Ok(Self {
            pipeline: Pipeline::new(tracks, segment_length)?,
            file: RecordingFile::create(path).await?,
        })
    }

    async fn write(&mut self, output: Output) -> Result<()> {
        if let Some(tail) = output.previous_file_tail {
            self.file.rotate(&tail).await?;
            tracing::info!(path = %self.file.path().display(), "recording into a new file");
        }
        self.file.write(&output.data).await?;
        if output.group_complete {
            self.file.flush().await?;
        }
        Ok(())
    }
}

fn track_path(path: &Path, track: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let track = track.replace('/', "_");
    let name = match path.extension() {
        Some(extension) => format!("{stem}-{track}.{}", extension.to_string_lossy()),
        None => format!("{stem}-{track}"),
    };
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use mediapack::{MediaEvent, loc::VideoCodec};

    use super::*;
    use crate::{
        test_support::{
            audio_samples, demux, first_video_capture, fixture, opus_config, opus_object,
            video_samples,
        },
        track_selection::{AudioTrack, VideoTrack},
    };

    fn selection() -> Selection {
        Selection {
            video: VideoTrack {
                name: "video/profile_1".to_string(),
                codec: VideoCodec::H264,
            },
            audio: Some(AudioTrack {
                name: "audio".to_string(),
                config: opus_config(),
            }),
        }
    }

    fn scratch_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("recorder-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Records the fixture's first two video objects and one audio frame 30 ms
    /// after the first video object.
    async fn record(layout: Layout, dir: &Path) -> Vec<PathBuf> {
        let fixture = fixture();
        let video_capture = first_video_capture(&fixture.video[0]);
        let mut recording = Recording::create(&selection(), layout, dir.join("rec.mp4"), None)
            .await
            .unwrap();
        let paths = recording.paths().map(Path::to_path_buf).collect();
        recording.push_video(&fixture.video[0]).await.unwrap();
        recording
            .push_audio(&opus_object(1, 0, video_capture + 30_000))
            .await
            .unwrap();
        recording.push_video(&fixture.video[1]).await.unwrap();
        recording.finish().await.unwrap();
        paths
    }

    #[tokio::test]
    async fn per_track_layout_writes_each_track_to_its_own_file_on_one_timeline() {
        // Arrange
        let dir = scratch_dir();

        // Act
        let paths = record(Layout::PerTrack, &dir).await;

        // Assert: the audio keeps its 30 ms offset to the video in the other file
        assert_eq!(
            paths,
            [
                dir.join("rec-video_profile_1.mp4"),
                dir.join("rec-audio.mp4")
            ]
        );
        let video = demux(&std::fs::read(&paths[0]).unwrap());
        let audio = demux(&std::fs::read(&paths[1]).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(video_samples(&video)[0].pts, Timestamp::ZERO);
        assert!(audio_samples(&video).is_empty());
        assert!(video_samples(&audio).is_empty());
        assert_eq!(audio_samples(&audio)[0].pts, Timestamp::from_millis(30));
        assert!(audio.contains(&MediaEvent::AudioConfig(opus_config())));
    }

    async fn record_combined(
        audio_before: &[TrackObject],
        audio_after: &[TrackObject],
    ) -> Vec<MediaEvent> {
        let fixture = fixture();
        let dir = scratch_dir();
        let mut recording =
            Recording::create(&selection(), Layout::Combined, dir.join("rec.mp4"), None)
                .await
                .unwrap();
        for audio in audio_before {
            recording.push_audio(audio).await.unwrap();
        }
        recording.push_video(&fixture.video[0]).await.unwrap();
        for audio in audio_after {
            recording.push_audio(audio).await.unwrap();
        }
        recording.push_video(&fixture.video[1]).await.unwrap();
        recording.finish().await.unwrap();
        let recorded = demux(&std::fs::read(dir.join("rec.mp4")).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        recorded
    }

    #[tokio::test]
    async fn audio_captured_before_the_first_video_object_is_dropped() {
        // Arrange: the audio subscription starts one group before the video's
        let start = first_video_capture(&fixture().video[0]);
        let audio = [
            opus_object(0, 0, start - 40_000),
            opus_object(0, 1, start - 20_000),
            opus_object(1, 0, start),
            opus_object(1, 1, start + 20_000),
        ];

        // Act
        let recorded = record_combined(&[], &audio).await;

        // Assert
        let pts: Vec<_> = audio_samples(&recorded)
            .iter()
            .map(|sample| sample.pts)
            .collect();
        assert_eq!(pts, [Timestamp::ZERO, Timestamp::from_millis(20)]);
    }

    #[tokio::test]
    async fn audio_arriving_before_the_first_video_object_keeps_its_offset() {
        // Arrange: the audio of the video's first group overtakes its keyframe
        let start = first_video_capture(&fixture().video[0]);

        // Act
        let recorded = record_combined(&[opus_object(1, 0, start + 30_000)], &[]).await;

        // Assert
        assert_eq!(video_samples(&recorded)[0].pts, Timestamp::ZERO);
        assert_eq!(audio_samples(&recorded)[0].pts, Timestamp::from_millis(30));
    }

    #[tokio::test]
    async fn audio_held_for_the_first_video_object_is_capped() {
        // Arrange: one frame more than the cap arrives before any video
        let start = first_video_capture(&fixture().video[0]);
        let audio: Vec<_> = (0..=MAX_PENDING_AUDIO as u64)
            .map(|index| opus_object(1, index, start + index * 20_000))
            .collect();

        // Act
        let recorded = record_combined(&audio, &[]).await;

        // Assert: the oldest held frame was dropped
        let audio = audio_samples(&recorded);
        assert_eq!(audio.len(), MAX_PENDING_AUDIO);
        assert_eq!(audio[0].pts, Timestamp::from_millis(20));
    }

    #[tokio::test]
    async fn combined_layout_writes_both_tracks_into_one_file() {
        // Arrange
        let dir = scratch_dir();

        // Act
        let paths = record(Layout::Combined, &dir).await;

        // Assert
        assert_eq!(paths, [dir.join("rec.mp4")]);
        let recorded = demux(&std::fs::read(&paths[0]).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(video_samples(&recorded).len(), 2);
        assert_eq!(audio_samples(&recorded).len(), 1);
    }
}
