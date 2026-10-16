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
    file_writer::{ClosedFile, RecordingFile},
    pipeline::{FileTracks, Output, Pipeline, capture_timestamp},
    track_selection::{AudioTrack, Selection},
    upload_task::{Upload, UploadQueue},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Layout {
    /// The video and the audio in one file.
    Combined,
    /// Each track in its own file, `<stem>-<track>.mp4`.
    PerTrack,
}

impl Layout {
    /// The value `--layout` and the recording API take.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Combined => "combined",
            Self::PerTrack => "per-track",
        }
    }
}

/// About 10 s of 20 ms frames, well past the one group the audio can lead
/// the video by. Beyond it the oldest held object is dropped: it is the most
/// likely to precede the video.
const MAX_PENDING_AUDIO: usize = 500;

/// Where the closed files of a recording go: object
/// `<prefix>/<track>/<started_at_ms>-<index>.mp4`, without `<track>` for the
/// combined file.
pub(crate) struct Uploader {
    pub(crate) queue: UploadQueue,
    pub(crate) prefix: object_store::path::Path,
    pub(crate) started_at_ms: u64,
}

/// The files of one recording. They share one capture origin, the capture
/// timestamp of the first video object, so their timelines line up. Each
/// track subscribes from its own next group, so the audio can start a group
/// before the video, and an audio object can overtake the video's first: the
/// audio waits for the origin, and audio captured before it is dropped.
pub(crate) struct Recording {
    /// The video's file is at index 0.
    files: Vec<TrackFile>,
    audio: Option<usize>,
    layout: Layout,
    path: PathBuf,
    segment_length: Option<Duration>,
    capture_origin: Option<Timestamp>,
    pending_audio: VecDeque<TrackObject>,
    uploader: Option<Uploader>,
}

struct TrackFile {
    pipeline: Pipeline,
    file: RecordingFile,
    /// The track the file holds alone; `None` for the combined file.
    track: Option<String>,
}

impl Recording {
    pub(crate) async fn create(
        selection: &Selection,
        layout: Layout,
        path: PathBuf,
        segment_length: Option<Duration>,
        uploader: Option<Uploader>,
    ) -> Result<Self> {
        let tracks = FileTracks {
            video: Some(selection.video.codec),
            audio: None,
        };
        let name = (layout == Layout::PerTrack).then_some(selection.video.name.as_str());
        let video = TrackFile::create(tracks, name, &path, segment_length).await?;
        let mut recording = Self {
            files: vec![video],
            audio: None,
            layout,
            path,
            segment_length,
            capture_origin: None,
            pending_audio: VecDeque::new(),
            uploader,
        };
        if let Some(audio) = &selection.audio {
            recording.add_audio(audio).await?;
        }
        Ok(recording)
    }

    /// Adds an audio track to a recording that reads none: the first, or one
    /// after an audio track ended. A later track goes to the file the ended one
    /// went to; the per-track layout opens a file only for the first. When the
    /// track starts in that file follows `Pipeline::add_audio`.
    pub(crate) async fn add_audio(&mut self, track: &AudioTrack) -> Result<()> {
        if let Some(index) = self.audio {
            return self.files[index].pipeline.add_audio(track.config.clone());
        }
        let index = match self.layout {
            Layout::Combined => {
                self.files[0].pipeline.add_audio(track.config.clone())?;
                0
            }
            Layout::PerTrack => {
                let tracks = FileTracks {
                    video: None,
                    audio: Some(track.config.clone()),
                };
                let file =
                    TrackFile::create(tracks, Some(&track.name), &self.path, self.segment_length)
                        .await?;
                self.files.push(file);
                self.files.len() - 1
            }
        };
        self.audio = Some(index);
        Ok(())
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.iter().map(|file| file.file.path())
    }

    pub(crate) async fn push_video(&mut self, object: &TrackObject) -> Result<()> {
        if self.capture_origin.is_none() {
            self.capture_origin = capture_timestamp(object);
        }
        let output = self.files[0]
            .pipeline
            .push_video(object, self.capture_origin)?;
        self.write(0, output).await?;
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
        let output = self.files[index]
            .pipeline
            .push_audio(object, Some(origin))?;
        self.write(index, output).await
    }

    async fn write(&mut self, index: usize, output: Output) -> Result<()> {
        let file = &mut self.files[index];
        if let (Some(closed), Some(uploader)) = (file.write(output).await?, &self.uploader) {
            uploader.upload(file.track.as_deref(), closed, self.capture_origin);
        }
        Ok(())
    }

    pub(crate) async fn finish(self) -> Result<()> {
        let Self {
            files,
            capture_origin,
            uploader,
            ..
        } = self;
        for mut file in files {
            let tail = file.pipeline.finish()?;
            let closed = file.file.finish(&tail).await?;
            if let Some(uploader) = &uploader {
                uploader.upload(file.track.as_deref(), closed, capture_origin);
            }
        }
        Ok(())
    }
}

impl Uploader {
    fn upload(&self, track: Option<&str>, closed: ClosedFile, capture_origin: Option<Timestamp>) {
        let mut key = self.prefix.clone();
        key.extend(track.iter().flat_map(|track| track.split('/')));
        key.extend([format!("{}-{}.mp4", self.started_at_ms, closed.index)]);
        self.queue.upload(Upload {
            path: closed.path,
            key,
            capture_origin,
        });
    }
}

impl TrackFile {
    async fn create(
        tracks: FileTracks,
        track: Option<&str>,
        path: &Path,
        segment_length: Option<Duration>,
    ) -> Result<Self> {
        let path = match track {
            Some(track) => track_path(path, track),
            None => path.to_path_buf(),
        };
        Ok(Self {
            pipeline: Pipeline::new(tracks, segment_length)?,
            file: RecordingFile::create(path).await?,
            track: track.map(str::to_string),
        })
    }

    /// The file `output` closed, if any.
    async fn write(&mut self, output: Output) -> Result<Option<ClosedFile>> {
        let mut closed = None;
        if let Some(tail) = output.previous_file_tail {
            closed = Some(self.file.rotate(&tail).await?);
            tracing::info!(path = %self.file.path().display(), "recording into a new file");
        }
        self.file.write(&output.data).await?;
        if output.group_complete {
            self.file.flush().await?;
        }
        Ok(closed)
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
    use std::sync::Arc;

    use mediapack::{MediaEvent, loc::VideoCodec};
    use object_store::{ObjectStoreExt, memory::InMemory};

    use crate::upload_task::UploadTask;

    use super::*;
    use crate::{
        test_support::{
            audio_samples, demux, first_video_capture, fixture, opus_config, opus_object,
            scratch_dir, video_samples,
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

    /// Records the fixture's first two video objects and one audio frame 30 ms
    /// after the first video object.
    async fn record(layout: Layout, dir: &Path, uploader: Option<Uploader>) -> Vec<PathBuf> {
        let fixture = fixture();
        let video_capture = first_video_capture(&fixture.video[0]);
        let mut recording =
            Recording::create(&selection(), layout, dir.join("rec.mp4"), None, uploader)
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
        let paths = record(Layout::PerTrack, &dir, None).await;

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
        let mut recording = Recording::create(
            &selection(),
            Layout::Combined,
            dir.join("rec.mp4"),
            None,
            None,
        )
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
    async fn an_audio_track_listed_after_the_recording_started_joins_the_combined_file() {
        // Arrange: the first catalog listed the audio track without its configuration
        let fixture = fixture();
        let start = first_video_capture(&fixture.video[0]);
        let dir = scratch_dir();
        let video_only = Selection {
            audio: None,
            ..selection()
        };
        let mut recording = Recording::create(
            &video_only,
            Layout::Combined,
            dir.join("rec.mp4"),
            None,
            None,
        )
        .await
        .unwrap();

        // Act: the next catalog lists it before the first video object arrives
        recording
            .add_audio(&selection().audio.unwrap())
            .await
            .unwrap();
        recording.push_video(&fixture.video[0]).await.unwrap();
        recording
            .push_audio(&opus_object(1, 0, start + 20_000))
            .await
            .unwrap();
        recording.push_video(&fixture.video[1]).await.unwrap();
        let paths: Vec<_> = recording.paths().map(Path::to_path_buf).collect();
        recording.finish().await.unwrap();

        // Assert
        let recorded = demux(&std::fs::read(dir.join("rec.mp4")).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(paths, [dir.join("rec.mp4")]);
        assert!(recorded.contains(&MediaEvent::AudioConfig(opus_config())));
        assert_eq!(video_samples(&recorded).len(), 2);
        assert_eq!(audio_samples(&recorded)[0].pts, Timestamp::from_millis(20));
    }

    /// Records the fixture's video with an audio frame 20 ms after the video
    /// start, then adds the same audio track again and records a frame 1 s
    /// after the video start.
    async fn record_mute_and_unmute(layout: Layout, dir: &Path) -> Vec<PathBuf> {
        let fixture = fixture();
        let start = first_video_capture(&fixture.video[0]);
        let (last, earlier) = fixture.video.split_last().unwrap();
        let mut recording =
            Recording::create(&selection(), layout, dir.join("rec.mp4"), None, None)
                .await
                .unwrap();
        recording.push_video(&earlier[0]).await.unwrap();
        recording
            .push_audio(&opus_object(1, 0, start + 20_000))
            .await
            .unwrap();
        for object in &earlier[1..] {
            recording.push_video(object).await.unwrap();
        }
        recording
            .add_audio(&selection().audio.unwrap())
            .await
            .unwrap();
        recording
            .push_audio(&opus_object(last.group_id, 0, start + 1_000_000))
            .await
            .unwrap();
        recording.push_video(last).await.unwrap();
        let paths = recording.paths().map(Path::to_path_buf).collect();
        recording.finish().await.unwrap();
        paths
    }

    #[tokio::test]
    async fn audio_listed_again_after_it_ended_resumes_in_the_combined_file() {
        // Arrange
        let dir = scratch_dir();

        // Act
        let paths = record_mute_and_unmute(Layout::Combined, &dir).await;

        // Assert: one file with every video sample, and the audio on both sides of the gap
        let recorded = demux(&std::fs::read(dir.join("rec.mp4")).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(paths, [dir.join("rec.mp4")]);
        assert_eq!(
            video_samples(&recorded).len(),
            video_samples(&fixture().events).len()
        );
        let audio: Vec<_> = audio_samples(&recorded)
            .iter()
            .map(|sample| sample.pts)
            .collect();
        assert_eq!(
            audio,
            [Timestamp::from_millis(20), Timestamp::from_millis(1_000)]
        );
    }

    #[tokio::test]
    async fn audio_listed_again_after_it_ended_goes_on_in_its_own_file() {
        // Arrange
        let dir = scratch_dir();

        // Act
        let paths = record_mute_and_unmute(Layout::PerTrack, &dir).await;

        // Assert: no second audio file
        let audio = demux(&std::fs::read(dir.join("rec-audio.mp4")).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            paths,
            [
                dir.join("rec-video_profile_1.mp4"),
                dir.join("rec-audio.mp4")
            ]
        );
        let audio: Vec<_> = audio_samples(&audio)
            .iter()
            .map(|sample| sample.pts)
            .collect();
        assert_eq!(
            audio,
            [Timestamp::from_millis(20), Timestamp::from_millis(1_000)]
        );
    }

    #[tokio::test]
    async fn combined_layout_writes_both_tracks_into_one_file() {
        // Arrange
        let dir = scratch_dir();

        // Act
        let paths = record(Layout::Combined, &dir, None).await;

        // Assert
        assert_eq!(paths, [dir.join("rec.mp4")]);
        let recorded = demux(&std::fs::read(&paths[0]).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(video_samples(&recorded).len(), 2);
        assert_eq!(audio_samples(&recorded).len(), 1);
    }

    #[tokio::test]
    async fn uploads_each_track_under_its_own_key_and_deletes_the_local_files() {
        // Arrange
        let dir = scratch_dir();
        let store = Arc::new(InMemory::new());
        let task = UploadTask::run(store.clone());
        let uploader = Uploader {
            queue: task.queue(),
            prefix: object_store::path::Path::from("recordings/anon/live"),
            started_at_ms: 1_700_000_000_000,
        };

        // Act
        let paths = record(Layout::PerTrack, &dir, Some(uploader)).await;
        task.finish().await;

        // Assert: a `/` in the track name nests the key
        let video = store
            .get(&"recordings/anon/live/video/profile_1/1700000000000-1.mp4".into())
            .await
            .unwrap();
        let audio = store
            .get(&"recordings/anon/live/audio/1700000000000-1.mp4".into())
            .await
            .unwrap();
        let video = demux(&video.bytes().await.unwrap());
        let audio = demux(&audio.bytes().await.unwrap());
        assert!(paths.iter().all(|path| !path.exists()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(video_samples(&video).len(), 2);
        assert_eq!(audio_samples(&audio).len(), 1);
    }
}
