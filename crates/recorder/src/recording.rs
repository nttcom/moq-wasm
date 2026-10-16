use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::ValueEnum;
use mediapack::Timestamp;
use moqt::TrackObject;

use crate::{
    file_writer::RecordingFile,
    pipeline::{FileTracks, Output, Pipeline},
    track_selection::Selection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Layout {
    /// The video and the audio in one file.
    Combined,
    /// Each track in its own file, `<stem>-<track>.mp4`.
    PerTrack,
}

/// The files of one recording. They share one capture origin, the first
/// capture timestamp any track delivers, so their timelines line up.
pub(crate) struct Recording {
    /// The video's file is at index 0.
    files: Vec<TrackFile>,
    audio: Option<usize>,
    capture_origin: Option<Timestamp>,
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
        })
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|file| file.file.path())
    }

    pub(crate) async fn push_video(&mut self, object: &TrackObject) -> Result<()> {
        let file = &mut self.files[0];
        let output = file.pipeline.push_video(object, &mut self.capture_origin)?;
        file.write(output).await
    }

    pub(crate) async fn push_audio(&mut self, object: &TrackObject) -> Result<()> {
        let index = self.audio.context("audio object without an audio track")?;
        let file = &mut self.files[index];
        let output = file.pipeline.push_audio(object, &mut self.capture_origin)?;
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
