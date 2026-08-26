use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    relay::cache::track_cache::TrackCache,
    session_event::{EventKind, SessionEvent},
    types::{SessionId, TrackKey},
};

/// Dropped (and thereby aborted) when ingress for the track stops, so the
/// held cache Arc never blocks eviction of the track.
pub(crate) struct MalformedTrackWatchTask {
    join_handle: JoinHandle<()>,
}

impl MalformedTrackWatchTask {
    pub(crate) fn run(
        cache: Arc<TrackCache>,
        track_key: TrackKey,
        publisher_session_id: SessionId,
        event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            cache.malformed_track_detected().await;
            if event_sender
                .send(SessionEvent {
                    session_id: publisher_session_id,
                    kind: EventKind::MalformedTrackDetected(track_key.clone()),
                })
                .is_err()
            {
                tracing::warn!(%track_key, "failed to report malformed track detection");
            }
        });
        Self { join_handle }
    }
}

impl Drop for MalformedTrackWatchTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};

    use super::*;
    use crate::modules::{core::data_object::DataObject, relay::types::StreamSubgroupId};

    fn make_object(payload: &'static [u8]) -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object: SubgroupObject::new_payload(Bytes::from_static(payload)),
        })
    }

    async fn latch_malformed(cache: &TrackCache) {
        let subgroup = StreamSubgroupId::Value(0);
        let _ = cache
            .append_stream_object(0, &subgroup, Some(0), make_object(b"a"))
            .await;
        let _ = cache
            .append_stream_object(0, &subgroup, Some(0), make_object(b"b"))
            .await;
        assert!(cache.is_malformed());
    }

    #[tokio::test]
    async fn reports_detection_to_the_event_pipeline() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let track_key = TrackKey::new("ns", "track");
        let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
        let _task = MalformedTrackWatchTask::run(cache.clone(), track_key.clone(), 7, event_sender);

        // Act
        latch_malformed(&cache).await;

        // Assert
        let event = tokio::time::timeout(Duration::from_secs(3), event_receiver.recv())
            .await
            .expect("watch task should report the detection")
            .expect("event channel should stay open");
        assert_eq!(event.session_id, 7);
        match event.kind {
            EventKind::MalformedTrackDetected(reported_track_key) => {
                assert_eq!(reported_track_key, track_key);
            }
            _ => panic!("expected MalformedTrackDetected"),
        }
    }

    #[tokio::test]
    async fn dropped_watcher_reports_nothing() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
        let task = MalformedTrackWatchTask::run(
            cache.clone(),
            TrackKey::new("ns", "track"),
            7,
            event_sender,
        );

        // Act: ingress stops the track before any detection.
        drop(task);
        latch_malformed(&cache).await;

        // Assert: the aborted watcher sends no event (sender dropped).
        assert!(
            tokio::time::timeout(Duration::from_secs(1), event_receiver.recv())
                .await
                .expect("channel should close once the watcher is dropped")
                .is_none()
        );
    }
}
