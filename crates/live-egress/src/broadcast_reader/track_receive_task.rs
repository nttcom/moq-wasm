use std::sync::{Arc, OnceLock};

use mediapack::{
    AudioConfig, MediaEvent, Timestamp,
    loc::{self, LocObject, from_extension_headers},
};
use moqt::{TrackObject, TrackReader};
use tokio::{sync::mpsc, task::JoinHandle};

pub(super) enum TrackKind {
    Video,
    Audio(AudioConfig),
}

/// Where the tracks of one broadcast start, shared by their receive tasks.
/// Each track subscribes from its own next group, so the audio can start one
/// group before the video. `broadcast` writes each audio object into the group
/// of the video keyframe before it, so the tracks share group ids and audio
/// is held to the groups at or after the video's first.
pub(super) struct StartPoint {
    has_video: bool,
    video_group: OnceLock<u64>,
    capture_origin: OnceLock<Timestamp>,
}

impl StartPoint {
    pub(super) fn new(has_video: bool) -> Self {
        Self {
            has_video,
            video_group: OnceLock::new(),
            capture_origin: OnceLock::new(),
        }
    }

    pub(super) fn capture_origin(&self) -> Option<Timestamp> {
        self.capture_origin.get().copied()
    }

    /// Audio that arrives before the first video object is dropped too: its
    /// group is not known to be the video's.
    fn admits(&self, kind: &TrackKind, group_id: u64) -> bool {
        match kind {
            TrackKind::Video => {
                self.video_group.get_or_init(|| group_id);
                true
            }
            TrackKind::Audio(_) => {
                !self.has_video
                    || self
                        .video_group
                        .get()
                        .is_some_and(|start| group_id >= *start)
            }
        }
    }
}

/// Reads one LOC track into the broadcast's media channel. The task ends
/// with the track or when the channel is closed.
pub(super) struct TrackReceiveTask {
    handle: JoinHandle<()>,
}

impl TrackReceiveTask {
    pub(super) fn run(
        name: String,
        mut reader: TrackReader,
        kind: TrackKind,
        start: Arc<StartPoint>,
        media_sender: mpsc::Sender<MediaEvent>,
    ) -> Self {
        let handle = tokio::spawn(async move {
            let mut demuxer = None;
            loop {
                let object = match reader.next_object().await {
                    Ok(Some(object)) => object,
                    Ok(None) => {
                        tracing::info!(track = %name, "track ended");
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(track = %name, %error, "group read error, advancing");
                        continue;
                    }
                };
                if !start.admits(&kind, object.group_id) {
                    continue;
                }
                let object = loc_object(object);
                let demuxer = demuxer.get_or_insert_with(|| {
                    let origin = *start
                        .capture_origin
                        .get_or_init(|| object.capture_timestamp().unwrap_or(Timestamp::ZERO));
                    match &kind {
                        TrackKind::Video => loc::Demuxer::video(),
                        TrackKind::Audio(config) => loc::Demuxer::audio(config.clone()),
                    }
                    .with_capture_origin(origin)
                });
                let events = match demuxer.push(&object) {
                    Ok(events) => events,
                    Err(error) => {
                        tracing::warn!(track = %name, %error, "object dropped");
                        continue;
                    }
                };
                for event in events {
                    // The reader announces the broadcast's streams once from
                    // the catalog; each track's own announcement is redundant.
                    if matches!(event, MediaEvent::Streams(_)) {
                        continue;
                    }
                    if media_sender.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        Self { handle }
    }
}

impl Drop for TrackReceiveTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn loc_object(object: TrackObject) -> LocObject {
    LocObject {
        extensions: from_extension_headers(&object.extension_headers),
        payload: object.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> TrackKind {
        TrackKind::Audio(AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        })
    }

    #[test]
    fn holds_audio_to_the_group_the_video_starts_at() {
        // Arrange
        let start = StartPoint::new(true);

        // Act
        let before_video = start.admits(&audio(), 9);
        let video = start.admits(&TrackKind::Video, 10);
        let earlier_group = start.admits(&audio(), 9);
        let same_group = start.admits(&audio(), 10);

        // Assert
        assert_eq!(
            [before_video, video, earlier_group, same_group],
            [false, true, false, true]
        );
    }

    #[test]
    fn passes_audio_of_a_broadcast_without_video() {
        // Arrange
        let start = StartPoint::new(false);

        // Act / Assert
        assert!(start.admits(&audio(), 3));
    }
}
