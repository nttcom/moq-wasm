use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::modules::extensions::mutex::lock;
use crate::modules::moqt::domains::namespace_watchers::NamespaceWatchers;
use crate::{
    RequestErrorCode, Session, SessionEvent,
    modules::{
        client::{
            event_handler::{EventHandler, HandlerId},
            subscription::PublishDone,
        },
        telemetry::join_trace,
    },
    publish::Origin,
};

type TrackKey = (String, String);

/// Where the session's events go. Each goes to the handlers first, in the
/// order they were added, then to `origin`, which answers the peer's
/// SUBSCRIBE, FETCH and TRACK_STATUS for the tracks the session publishes,
/// then to the namespace watchers. A SUBSCRIBE for any other track is
/// rejected with TRACK_DOES_NOT_EXIST and every PUBLISH with UNINTERESTED;
/// PUBLISH_DONE goes to the waiter of its subscription.
///
/// The tables are `std::sync::Mutex`es, never held across an `.await`.
#[derive(Default)]
pub(super) struct Routes {
    pub(super) origin: Origin,
    handlers: Mutex<Vec<(HandlerId, Arc<dyn EventHandler>)>>,
    next_handler_id: AtomicU64,
    pub(super) watchers: NamespaceWatchers,
    publish_done: Mutex<HashMap<u64, oneshot::Sender<PublishDone>>>,
    subscriber_watchers: Mutex<HashMap<TrackKey, mpsc::UnboundedSender<()>>>,
}

impl Routes {
    pub(super) fn add_handler(&self, handler: Arc<dyn EventHandler>) -> HandlerId {
        let id = HandlerId(self.next_handler_id.fetch_add(1, Ordering::Relaxed));
        lock(&self.handlers).push((id, handler));
        id
    }

    pub(super) fn remove_handler(&self, id: HandlerId) {
        lock(&self.handlers).retain(|(added, _)| *added != id);
    }

    /// Fails once the session ends or `forget` drops it.
    pub(super) fn publish_done(&self, request_id: u64) -> oneshot::Receiver<PublishDone> {
        let (sender, receiver) = oneshot::channel();
        lock(&self.publish_done).insert(request_id, sender);
        receiver
    }

    pub(super) fn forget(&self, request_id: u64) {
        lock(&self.publish_done).remove(&request_id);
    }

    /// Receives once per SUBSCRIBE `origin` answers for the track; ends with
    /// the session. Watching a track again replaces the earlier watch.
    pub(super) fn watch_subscribers(
        &self,
        namespace: &str,
        track: &str,
    ) -> mpsc::UnboundedReceiver<()> {
        let (sender, receiver) = mpsc::unbounded_channel();
        lock(&self.subscriber_watchers).insert((namespace.to_string(), track.to_string()), sender);
        receiver
    }

    /// Returns once the session has ended.
    pub(super) async fn route(&self, session: &Session) {
        while let Ok(event) = session.receive_event().await {
            let Some(event) = self.pass_handlers(event).await else {
                continue;
            };
            let subscribed = match &event {
                SessionEvent::Subscribe(handler) => {
                    Some((handler.track_namespace.clone(), handler.track_name.clone()))
                }
                _ => None,
            };
            let Some(event) = self.origin.handle(event).await else {
                if let Some(track) = subscribed {
                    self.notify_subscribed(&track);
                }
                continue;
            };
            let Some(event) = self.watchers.handle(event).await else {
                continue;
            };
            if !self.answer(event).await {
                break;
            }
        }
        self.end();
    }

    pub(super) fn end(&self) {
        self.watchers.close();
        lock(&self.publish_done).clear();
        lock(&self.subscriber_watchers).clear();
    }

    async fn pass_handlers(&self, event: SessionEvent) -> Option<SessionEvent> {
        let handlers: Vec<_> = lock(&self.handlers)
            .iter()
            .map(|(_, handler)| handler.clone())
            .collect();
        let mut event = event;
        for handler in handlers {
            event = handler.handle(event).await?;
        }
        Some(event)
    }

    /// Answers what nothing before took; `false` once the session ended.
    async fn answer(&self, event: SessionEvent) -> bool {
        match event {
            SessionEvent::PublishDone(handler) => {
                let sender = lock(&self.publish_done).remove(&handler.request_id());
                if let Some(sender) = sender {
                    let _ = sender.send(PublishDone {
                        code: handler.status_code(),
                        reason: handler.error_reason().to_string(),
                    });
                }
            }
            SessionEvent::SubscribeNameSpace(handler) => {
                let span = tracing::info_span!(
                    parent: None,
                    "moqt.client.subscribe_namespace",
                    track_namespace_prefix = %handler.track_namespace_prefix,
                );
                join_trace(&span, handler.trace_context());
                async {
                    if let Err(error) = handler.ok().await {
                        tracing::debug!(%error, "failed to acknowledge SUBSCRIBE_NAMESPACE");
                    }
                }
                .instrument(span)
                .await;
            }
            SessionEvent::Subscribe(handler) => {
                let span = tracing::info_span!(
                    parent: None,
                    "moqt.client.subscribe",
                    request_id = handler.request_id(),
                    track_namespace = %handler.track_namespace,
                    track_name = %handler.track_name,
                );
                join_trace(&span, handler.trace_context());
                let reason = format!("{} is not published here", handler.track_name);
                let _ = handler
                    .error(RequestErrorCode::TrackDoesNotExist, reason)
                    .instrument(span)
                    .await;
            }
            SessionEvent::Publish(handler) => {
                let span = tracing::info_span!(
                    parent: None,
                    "moqt.client.publish",
                    request_id = handler.request_id,
                    track_namespace = %handler.track_namespace,
                    track_name = %handler.track_name,
                );
                join_trace(&span, handler.trace_context());
                let _ = handler
                    .error(
                        RequestErrorCode::Uninterested,
                        "this session takes no PUBLISH".to_string(),
                    )
                    .instrument(span)
                    .await;
            }
            SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => return false,
            event => tracing::debug!(?event, "session event ignored"),
        }
        true
    }

    fn notify_subscribed(&self, track: &TrackKey) {
        if let Some(sender) = lock(&self.subscriber_watchers).get(track) {
            let _ = sender.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, sync::atomic::AtomicUsize};

    use async_trait::async_trait;
    use futures::FutureExt;
    use tracing::Span;

    use super::*;
    use crate::{
        RequestFailure, Subscription,
        client::{PublishTrackOptions, Session as ClientSession, SubscribeOptions},
        modules::{
            moqt::control_plane::control_messages::messages::parameters::trace_context::TEST_TRACEPARENT,
            test_support::{
                capture_telemetry, next_event, provide_trace_context,
                spawn_connected_native_sessions, within,
            },
        },
        object::ContentExists,
        publish::{PublishDoneCode, PublishOption},
        set_trace_context_joiner,
        subscribe::SubscribeOption,
    };

    async fn routed_sessions(name: &str) -> (ClientSession, Session) {
        let (client, server) = spawn_connected_native_sessions(name).await;
        (ClientSession::new(client), server)
    }

    async fn subscribe_from(server: &Session, track: &str) -> Result<Subscription, RequestFailure> {
        within(server.subscriber().subscribe(
            "ns".to_string(),
            track.to_string(),
            SubscribeOption::default(),
        ))
        .await
    }

    fn rejection_code<T>(result: Result<T, RequestFailure>) -> Option<RequestErrorCode> {
        match result {
            Err(RequestFailure::Rejected { code, .. }) => Some(code),
            _ => None,
        }
    }

    struct RejectSubscribes;

    #[async_trait]
    impl EventHandler for RejectSubscribes {
        async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
            let SessionEvent::Subscribe(handler) = event else {
                return Some(event);
            };
            let _ = handler
                .error(RequestErrorCode::Unauthorized, "rejected".to_string())
                .await;
            None
        }
    }

    #[derive(Default)]
    struct CountEvents(AtomicUsize);

    #[async_trait]
    impl EventHandler for CountEvents {
        async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(event)
        }
    }

    #[tokio::test]
    async fn publish_done_reaches_the_subscription_s_done() {
        // Arrange
        let (client, server) = routed_sessions("client-publish-done").await;
        let (subscription, served) = tokio::join!(
            within(client.subscribe(&["ns"], "track", SubscribeOptions::default())),
            async {
                let SessionEvent::Subscribe(handler) = next_event(&server).await else {
                    panic!("expected SUBSCRIBE");
                };
                let alias = handler.ok(0, ContentExists::False).await.unwrap();
                handler.into_subscription(alias)
            }
        );
        let subscription = subscription.unwrap();

        // Act
        server
            .publisher()
            .publish_done(
                &served,
                PublishDoneCode::TrackEnded,
                0,
                "finished".to_string(),
            )
            .await
            .unwrap();
        let done = within(subscription.done()).await;

        // Assert
        assert_eq!(
            done,
            Some(PublishDone {
                code: PublishDoneCode::TrackEnded,
                reason: "finished".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn a_subscribe_the_origin_answers_reaches_the_track_s_next_subscriber() {
        // Arrange
        let (client, server) = routed_sessions("client-next-subscriber").await;
        let writer = client
            .publish_track(&["ns"], "track", PublishTrackOptions::default())
            .await
            .unwrap();
        let other_track = client
            .publish_track(&["ns"], "other", PublishTrackOptions::default())
            .await
            .unwrap();

        // Act
        subscribe_from(&server, "track").await.unwrap();
        let subscribed = within(writer.next_subscriber()).await;

        // Assert: a writer of another track hears nothing
        assert!(subscribed);
        assert_eq!(other_track.next_subscriber().now_or_never(), None);
    }

    #[tokio::test]
    async fn a_subscribe_for_a_track_not_published_here_is_rejected_as_track_does_not_exist() {
        // Arrange
        let (_client, server) = routed_sessions("client-reject-subscribe").await;

        // Act
        let subscribed = subscribe_from(&server, "track").await;

        // Assert
        assert_eq!(
            rejection_code(subscribed),
            Some(RequestErrorCode::TrackDoesNotExist)
        );
    }

    #[tokio::test]
    async fn a_handler_takes_a_subscribe_before_the_origin_until_it_is_removed() {
        // Arrange
        let (client, server) = routed_sessions("client-handler-order").await;
        client
            .publish_track(&["ns"], "track", PublishTrackOptions::default())
            .await
            .unwrap();
        let handler = client.add_handler(Arc::new(RejectSubscribes));

        // Act
        let while_added = subscribe_from(&server, "track").await;
        client.remove_handler(handler);
        let after_removal = subscribe_from(&server, "track").await;

        // Assert
        assert_eq!(
            rejection_code(while_added),
            Some(RequestErrorCode::Unauthorized)
        );
        assert!(after_removal.is_ok());
    }

    #[tokio::test]
    async fn an_event_every_handler_returns_reaches_the_origin() {
        // Arrange
        let (client, server) = routed_sessions("client-handler-pass").await;
        client
            .publish_track(&["ns"], "track", PublishTrackOptions::default())
            .await
            .unwrap();
        let first = Arc::new(CountEvents::default());
        let second = Arc::new(CountEvents::default());
        client.add_handler(first.clone());
        client.add_handler(second.clone());

        // Act
        let subscribed = subscribe_from(&server, "track").await;

        // Assert
        assert!(subscribed.is_ok());
        assert_eq!(first.0.load(Ordering::Relaxed), 1);
        assert_eq!(second.0.load(Ordering::Relaxed), 1);
    }

    thread_local! {
        static JOINED: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }

    /// The joiner is process-global and set once, so it records on the
    /// thread that called it, which runs the event loop on a current-thread
    /// runtime.
    fn record_join(span: &Span, traceparent: &str) {
        let name = span.metadata().map_or("", |metadata| metadata.name());
        JOINED.with_borrow_mut(|joined| joined.push((name.to_string(), traceparent.to_string())));
    }

    #[tokio::test]
    async fn a_publish_is_rejected_as_uninterested_in_a_span_that_joins_the_peer_s_trace() {
        // Arrange
        let (_telemetry, _default) = capture_telemetry();
        provide_trace_context();
        set_trace_context_joiner(record_join);
        let (_client, server) = routed_sessions("client-publish-trace").await;

        // Act
        let published = within(server.publisher().publish(
            "ns".to_string(),
            "track".to_string(),
            PublishOption::default(),
        ))
        .await;

        // Assert
        assert_eq!(
            rejection_code(published),
            Some(RequestErrorCode::Uninterested)
        );
        assert_eq!(
            JOINED.with_borrow(Clone::clone),
            [(
                "moqt.client.publish".to_string(),
                TEST_TRACEPARENT.to_string()
            )]
        );
    }
}
