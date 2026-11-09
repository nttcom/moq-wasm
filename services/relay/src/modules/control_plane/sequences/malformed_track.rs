use crate::modules::{
    control_plane::control_message_forwarder::ControlMessageForwarder,
    directory::InMemoryLocalPubSubDirectory, domain::track_key::TrackKey,
};

pub(crate) struct MalformedTrackCleanup;

/// Whether `error` failed a request because its track is malformed
/// (draft-18 §2.4.2): moqt refuses a SUBSCRIBE_OK or FETCH_OK whose
/// Immutable Properties are malformed (§12.7) with MALFORMED_TRACK, as does a
/// relay that detected the track malformed.
pub(crate) fn refused_as_malformed(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<moqt::RequestFailure>(),
        Some(moqt::RequestFailure::Rejected {
            code: moqt::RequestErrorCode::MalformedTrack,
            ..
        })
    )
}

impl MalformedTrackCleanup {
    pub(crate) async fn handle(
        &self,
        track_key: &TrackKey,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
    ) {
        let released = table.remove_upstream_track(track_key);
        // draft-14 §2.5: the cache latch makes the whole track malformed, so
        // every publisher's subscription is ended, not only the reporting one.
        for (publisher_session_id, producer) in released {
            super::release_upstream(
                forwarder,
                publisher_session_id,
                &producer.state.subscription,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        directory::track::ProducerOrigin,
        domain::session_id::SessionId,
        test_support::directory_fixtures::{
            UPSTREAM_REQUEST_ID, UpstreamReleaseContext, upstream_release_context,
        },
    };

    async fn run_cleanup(ctx: &UpstreamReleaseContext) {
        MalformedTrackCleanup
            .handle(&TrackKey::new("ns", "track"), &ctx.table, &ctx.forwarder)
            .await;
    }

    #[tokio::test]
    async fn detection_unsubscribes_upstream_and_stops_ingress() {
        // Arrange
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;

        // Act
        run_cleanup(&ctx).await;

        // Assert
        assert!(ctx.table.upstream_tracks.is_empty());
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;
    }

    #[tokio::test]
    async fn duplicate_detection_reports_are_idempotent() {
        // Arrange: the first report already ran the cleanup.
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;
        run_cleanup(&ctx).await;
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;

        // Act: a second reader reports the same detection.
        run_cleanup(&ctx).await;

        // Assert
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert_eq!(ctx.recorded.ingests_stopped(), vec![UPSTREAM_REQUEST_ID]);
    }

    #[tokio::test]
    async fn detection_ends_the_upstream_subscription_of_every_publisher() {
        // Arrange
        const OTHER_PUBLISHER_SESSION: SessionId = 3;
        const OTHER_UPSTREAM_REQUEST_ID: u64 = 43;
        let ctx = upstream_release_context(ProducerOrigin::Subscribe).await;
        let other = ctx
            .add_publisher(OTHER_PUBLISHER_SESSION, OTHER_UPSTREAM_REQUEST_ID)
            .await;

        // Act
        run_cleanup(&ctx).await;

        // Assert
        assert!(ctx.table.upstream_tracks.is_empty());
        ctx.recorded.wait_ingest_stopped(UPSTREAM_REQUEST_ID).await;
        other.wait_ingest_stopped(OTHER_UPSTREAM_REQUEST_ID).await;
        assert_eq!(
            ctx.recorded.unsubscribed_request_ids(),
            vec![UPSTREAM_REQUEST_ID]
        );
        assert_eq!(
            other.unsubscribed_request_ids(),
            vec![OTHER_UPSTREAM_REQUEST_ID]
        );
    }
}
