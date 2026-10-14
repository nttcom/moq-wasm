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
        capture_origin: Arc<OnceLock<Timestamp>>,
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
                let object = loc_object(object);
                let demuxer = demuxer.get_or_insert_with(|| {
                    let origin = *capture_origin
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
