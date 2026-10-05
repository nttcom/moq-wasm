use moqt::ContentExists;
use tracing::Span;

use crate::modules::{
    core::handler::track_status::TrackStatusHandler,
    enums::SubscribeErrorCode,
    relay::cache::store::TrackCacheStore,
    sequences::{subscribe::cached_largest, tables::hashmap_table::InMemoryLocalPubSubDirectory},
    types::SessionId,
};

pub(crate) struct TrackStatus;

impl TrackStatus {
    /// draft-14 §9.20 lets a relay without an active subscription forward the
    /// request or subscribe upstream (MAY); this relay does neither and only
    /// reports tracks it is already subscribed to.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.track_status",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
        handler: &dyn TrackStatusHandler,
    ) {
        let response = match table
            .find_active_upstream_subscription(handler.track_namespace(), handler.track_name())
        {
            Some((_, active_upstream)) => {
                let content_exists = match cached_largest(cache_store, &active_upstream.track_key) {
                    Some(location) => ContentExists::True { location },
                    None => active_upstream.content_exists,
                };
                tracing::debug!(?content_exists, "answering TRACK_STATUS");
                handler
                    .ok(active_upstream.expires.unwrap_or(0), content_exists)
                    .await
            }
            None => {
                handler
                    .error(
                        SubscribeErrorCode::NotSupported as u64,
                        "track status is only known for subscribed tracks".to_string(),
                    )
                    .await
            }
        };
        if let Err(error) = response {
            tracing::warn!(?error, "failed to answer TRACK_STATUS");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use moqt::wire::AuthorizationToken;

    use super::*;
    use crate::modules::{
        relay::tests::harness::fixtures::{cached_object::insert_closed_group, location},
        sequences::{
            tables::table::UpstreamSubscriptionOrigin, test_fixtures::table_with_upstream,
        },
        types::TrackKey,
    };

    #[derive(Debug, PartialEq)]
    enum Response {
        Ok {
            expires: u64,
            content_exists: ContentExists,
        },
        Error(u64),
    }

    #[derive(Default)]
    struct MockTrackStatusHandler {
        responses: Mutex<Vec<Response>>,
    }

    impl MockTrackStatusHandler {
        fn responses(&self) -> Vec<Response> {
            std::mem::take(&mut self.responses.lock().unwrap())
        }
    }

    #[async_trait::async_trait]
    impl TrackStatusHandler for MockTrackStatusHandler {
        fn request_id(&self) -> u64 {
            7
        }

        fn track_namespace(&self) -> &str {
            "ns"
        }

        fn track_namespace_tuple(&self) -> &[String] {
            &[]
        }

        fn track_name(&self) -> &str {
            "track"
        }

        fn authorization_tokens(&self) -> &[AuthorizationToken] {
            &[]
        }

        async fn ok(
            &self,
            expires: u64,
            content_exists: ContentExists,
        ) -> Result<(), moqt::TransportSendError> {
            self.responses.lock().unwrap().push(Response::Ok {
                expires,
                content_exists,
            });
            Ok(())
        }

        async fn error(
            &self,
            code: u64,
            _reason_phrase: String,
        ) -> Result<(), moqt::TransportSendError> {
            self.responses.lock().unwrap().push(Response::Error(code));
            Ok(())
        }
    }

    #[tokio::test]
    async fn subscribed_track_reports_the_largest_cached_location() {
        // Arrange
        let (table, _) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let cache_store = TrackCacheStore::new();
        let cache = cache_store.get_or_create(&TrackKey::new("ns", "track"));
        insert_closed_group(&cache, 4, &[0, 1]);
        insert_closed_group(&cache, 5, &[0]);
        let handler = MockTrackStatusHandler::default();

        // Act
        TrackStatus
            .handle(1, &Span::none(), &table, &cache_store, &handler)
            .await;

        // Assert
        assert_eq!(
            handler.responses(),
            vec![Response::Ok {
                expires: 0,
                content_exists: ContentExists::True {
                    location: location(5, 0)
                },
            }]
        );
    }

    #[tokio::test]
    async fn subscribed_track_without_cached_objects_reports_the_upstream_status() {
        // Arrange
        let (table, _) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let handler = MockTrackStatusHandler::default();

        // Act
        TrackStatus
            .handle(1, &Span::none(), &table, &TrackCacheStore::new(), &handler)
            .await;

        // Assert
        assert_eq!(
            handler.responses(),
            vec![Response::Ok {
                expires: 0,
                content_exists: ContentExists::False,
            }]
        );
    }

    #[tokio::test]
    async fn track_without_an_active_subscription_is_not_supported() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let handler = MockTrackStatusHandler::default();

        // Act
        TrackStatus
            .handle(1, &Span::none(), &table, &TrackCacheStore::new(), &handler)
            .await;

        // Assert
        assert_eq!(
            handler.responses(),
            vec![Response::Error(SubscribeErrorCode::NotSupported as u64)]
        );
    }
}
