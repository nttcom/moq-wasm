use tokio::task::JoinSet;
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::cached_object::CachedObject,
        ingress::{
            datagram_reader::DatagramGroup,
            stream_reader::{read_stream, report_malformed_track},
            track_ingest_task::TrackIngest,
        },
    },
    session::data_receiver::receiver::{DataReceiver, UpstreamData},
};

/// Ingests one upstream subscription: each subgroup stream is read in a task
/// of its own, the datagrams here, in arrival order.
pub(crate) async fn read_track(
    mut ingest: TrackIngest,
    mut receiver: Box<dyn DataReceiver>,
    track_span: Span,
) {
    ingest.stamp.status_may_change = !receiver.compares_duplicate_status();
    let cache = ingest.cache.clone();
    let mut datagram_group = DatagramGroup::new(&cache);
    let mut readers = JoinSet::new();
    loop {
        tokio::select! {
            _ = ingest.stop_receiver.changed() => {
                tracing::info!(track_key = %ingest.track_key, "track ingress stopped");
                break;
            }
            data = receiver.next() => match data {
                Ok(UpstreamData::Subgroup(stream)) => {
                    let span = tracing::info_span!(
                        parent: &track_span,
                        "relay.dataplane.ingress.stream",
                        track_key = %ingest.track_key,
                        group_id = tracing::field::Empty,
                        subgroup_id = tracing::field::Empty,
                        end_reason = tracing::field::Empty,
                    );
                    readers.spawn(read_stream(ingest.clone(), stream).instrument(span));
                }
                Ok(UpstreamData::Datagram(object)) => {
                    let object = ingest.stamp.apply(CachedObject::from_object(object));
                    if datagram_group.insert(object).is_err() {
                        report_malformed_track(
                            &Span::current(),
                            &ingest.session_event_sender,
                            ingest.publisher_session_id,
                            &ingest.track_key,
                        );
                        break;
                    }
                }
                Err(error) => {
                    if error.is::<moqt::object::MalformedTrackError>() {
                        cache.mark_malformed();
                        report_malformed_track(
                            &Span::current(),
                            &ingest.session_event_sender,
                            ingest.publisher_session_id,
                            &ingest.track_key,
                        );
                    } else {
                        tracing::debug!(track_key = %ingest.track_key, %error, "upstream subscription ended");
                    }
                    break;
                }
            },
            Some(result) = readers.join_next() => {
                if let Err(e) = result {
                    tracing::error!("stream read task panicked: {:?}", e);
                }
            }
        }
    }
    readers.detach_all();
}

#[cfg(test)]
mod tests {
    use crate::modules::{
        session::session_event::EventKind,
        test_support::relay_harness::{FailingUpstream, RelayHarness},
    };

    #[tokio::test]
    async fn a_malformed_upstream_reports_the_malformed_track() {
        // Arrange
        let mut harness = RelayHarness::new();
        // Act
        harness
            .run_ingest(FailingUpstream(|| {
                moqt::object::MalformedTrackError::MixedForwardingPreference.into()
            }))
            .await;
        // Assert
        harness.wait_track_malformed().await;
        let event = harness.expect_session_event().await;
        assert!(matches!(event.kind, EventKind::MalformedTrackDetected(_)));
    }

    #[tokio::test]
    async fn an_ended_upstream_subscription_does_not_mark_the_track_malformed() {
        // Arrange
        let harness = RelayHarness::new();
        // Act
        harness
            .run_ingest(FailingUpstream(|| {
                anyhow::anyhow!("the subscription ended")
            }))
            .await;
        // Assert
        assert!(!harness.is_track_malformed());
    }
}
