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
/// The audio usually starts before the video, so it is held to the capture
/// time the video starts at: group ids cannot line the tracks up, since a
/// publisher that sends its audio as one endless group numbers it apart from
/// the video.
pub(super) struct StartPoint {
    has_video: bool,
    capture_origin: OnceLock<Timestamp>,
}

impl StartPoint {
    pub(super) fn new(has_video: bool) -> Self {
        Self {
            has_video,
            capture_origin: OnceLock::new(),
        }
    }

    pub(super) fn capture_origin(&self) -> Option<Timestamp> {
        self.capture_origin.get().copied()
    }

    /// The first admitted object sets the capture origin.
    fn admits(&self, kind: &TrackKind, object: &LocObject) -> bool {
        let capture = object.capture_timestamp();
        if matches!(kind, TrackKind::Video) || !self.has_video {
            self.capture_origin
                .get_or_init(|| capture.unwrap_or(Timestamp::ZERO));
            return true;
        }
        self.capture_origin
            .get()
            .is_some_and(|origin| capture.is_none_or(|capture| capture >= *origin))
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
                        tracing::warn!(track = %name, %error, "track is malformed");
                        break;
                    }
                };
                let object = loc_object(object);
                if !start.admits(&kind, &object) {
                    continue;
                }
                let demuxer = demuxer.get_or_insert_with(|| {
                    let origin = start
                        .capture_origin()
                        .expect("an admitted object sets the capture origin");
                    match &kind {
                        TrackKind::Video => loc::Demuxer::video(loc::VideoCodec::H264),
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
    use bytes::Bytes;

    use super::*;

    fn audio() -> TrackKind {
        TrackKind::Audio(AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        })
    }

    fn object(capture_micros: Option<u64>) -> LocObject {
        LocObject {
            extensions: capture_micros
                .map(|micros| loc::LocExtension::varint(loc::TIMESTAMP_ID, micros))
                .into_iter()
                .collect(),
            payload: Bytes::new(),
        }
    }

    #[test]
    fn holds_audio_to_the_capture_time_the_video_starts_at() {
        // Arrange
        let start = StartPoint::new(true);

        // Act
        let before_video = start.admits(&audio(), &object(Some(900)));
        let video = start.admits(&TrackKind::Video, &object(Some(1_000)));
        let captured_earlier = start.admits(&audio(), &object(Some(999)));
        let captured_with_video = start.admits(&audio(), &object(Some(1_000)));
        let unstamped = start.admits(&audio(), &object(None));

        // Assert
        assert_eq!(
            [
                before_video,
                video,
                captured_earlier,
                captured_with_video,
                unstamped
            ],
            [false, true, false, true, true]
        );
        assert_eq!(start.capture_origin(), Some(Timestamp::from_micros(1_000)));
    }

    #[test]
    fn passes_audio_of_a_broadcast_without_video() {
        // Arrange
        let start = StartPoint::new(false);

        // Act
        let first = start.admits(&audio(), &object(Some(3_000)));
        let earlier = start.admits(&audio(), &object(Some(2_000)));

        // Assert
        assert!(first && earlier);
        assert_eq!(start.capture_origin(), Some(Timestamp::from_micros(3_000)));
    }
}
