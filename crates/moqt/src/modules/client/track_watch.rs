use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc, watch};

use crate::{
    RequestErrorCode, Session, SessionEvent,
    modules::{
        client::{
            event_handler::{EventHandler, HandlerId},
            routes::Routes,
            session::GroupStartOptions,
            subscription::{Reading, Subscription},
        },
        moqt::domains::track_reader::TrackReader,
    },
    namespace::NamespaceEvent,
    publish::PublishHandler,
    subscribe::FilterType,
};

/// What `TrackWatch::next` returns, in the order the peer sent it.
pub enum WatchEvent {
    Namespace(NamespaceEvent),
    Track(PublishRequest),
}

/// The namespaces and the tracks published under one SUBSCRIBE_NAMESPACE
/// prefix (draft-14 §9.28), until the session ends or `close`: each
/// PUBLISH_NAMESPACE and PUBLISH_NAMESPACE_DONE, and each PUBLISH, which
/// the caller accepts or rejects.
pub struct TrackWatch {
    session: Arc<Session>,
    routes: Arc<Routes>,
    prefix: String,
    handler: HandlerId,
    events: Mutex<mpsc::UnboundedReceiver<Watched>>,
    /// Ends a `next` that waits, since it holds `events` meanwhile.
    closed: watch::Sender<bool>,
}

impl TrackWatch {
    // On wasm32 the PUBLISH handler is !Send, but wasm32 runs on one thread.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(super) fn new(session: Arc<Session>, routes: Arc<Routes>, prefix: Vec<String>) -> Self {
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let joined = prefix.join("/");
        let handler = routes.add_handler(Arc::new(WatchTaker {
            prefix,
            event_sender,
        }));
        Self {
            session,
            routes,
            prefix: joined,
            handler,
            events: Mutex::new(event_receiver),
            closed: watch::channel(false).0,
        }
    }

    /// The next namespace published or withdrawn, or track published; `None`
    /// once the session ends or `close` is called.
    pub async fn next(&self) -> Option<WatchEvent> {
        let mut closed = self.closed.subscribe();
        let mut events = self.events.lock().await;
        let watched = tokio::select! {
            biased;
            _ = closed.wait_for(|closed| *closed) => None,
            watched = events.recv() => watched,
        }?;
        Some(match watched {
            Watched::Namespace(event) => WatchEvent::Namespace(event),
            Watched::Publish(handler) => WatchEvent::Track(PublishRequest {
                session: self.session.clone(),
                routes: self.routes.clone(),
                handler,
            }),
        })
    }

    /// UNSUBSCRIBE_NAMESPACE (draft-14 §9.31); a `next` that waits returns
    /// `None`.
    pub async fn close(&self) -> anyhow::Result<()> {
        self.closed.send_replace(true);
        self.events.lock().await.close();
        self.session
            .subscriber()
            .unsubscribe_namespace(self.prefix.clone())
            .await
    }
}

impl Drop for TrackWatch {
    fn drop(&mut self) {
        self.routes.remove_handler(self.handler);
    }
}

/// A PUBLISH (draft-14 §9.13) of a track under a `TrackWatch`'s prefix.
/// Dropping it unanswered rejects it.
pub struct PublishRequest {
    session: Arc<Session>,
    routes: Arc<Routes>,
    handler: Box<PublishHandler>,
}

impl PublishRequest {
    pub fn namespace(&self) -> &[String] {
        &self.handler.track_namespace_tuple
    }

    pub fn track(&self) -> &str {
        &self.handler.track_name
    }

    /// PUBLISH_OK with Largest Object, read from the first object of the
    /// group the largest object is in: the PUBLISH's Largest Location is
    /// fetched with a Standalone FETCH and its objects come before the live
    /// ones. Fails when the PUBLISH_OK cannot be sent.
    pub async fn accept_current_group(
        mut self,
        options: GroupStartOptions,
    ) -> anyhow::Result<Subscription> {
        let option = options.subscribe_option();
        self.handler.delivery_timeout = option.delivery_timeout;
        self.handler.group_order = option.group_order;
        let subscription = self
            .handler
            .ok(
                option.subscriber_priority,
                FilterType::LargestObject,
                self.handler.forward,
            )
            .await?;
        self.handler.accept_data_receiver().await;
        let reader =
            TrackReader::fetching_current_group(self.session.subscriber(), subscription.clone());
        Ok(Subscription::new(
            self.session,
            self.routes,
            subscription,
            Reading::Objects(reader),
        ))
    }

    /// PUBLISH_ERROR with UNINTERESTED.
    pub async fn reject(self) -> anyhow::Result<()> {
        self.handler
            .error(
                RequestErrorCode::Uninterested,
                "the track is not read".to_string(),
            )
            .await?;
        Ok(())
    }
}

enum Watched {
    Namespace(NamespaceEvent),
    Publish(Box<PublishHandler>),
}

/// Reports PUBLISH_NAMESPACE and PUBLISH_NAMESPACE_DONE under the prefix and
/// hands them on, for the namespace watchers to acknowledge; takes PUBLISH
/// under the prefix while the watch lives.
struct WatchTaker {
    prefix: Vec<String>,
    event_sender: mpsc::UnboundedSender<Watched>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl EventHandler for WatchTaker {
    async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        let namespace = match &event {
            SessionEvent::PublishNamespace(handler) => {
                Some((handler.track_namespace_tuple.clone(), true))
            }
            SessionEvent::PublishNamespaceDone(handler) => {
                Some((handler.track_namespace_tuple.clone(), false))
            }
            _ => None,
        };
        if let Some((namespace, active)) = namespace {
            if namespace.starts_with(&self.prefix) {
                let _ = self
                    .event_sender
                    .send(Watched::Namespace(NamespaceEvent { namespace, active }));
            }
            return Some(event);
        }
        match event {
            SessionEvent::Publish(handler)
                if handler.track_namespace_tuple.starts_with(&self.prefix)
                    && !self.event_sender.is_closed() =>
            {
                let _ = self.event_sender.send(Watched::Publish(Box::new(handler)));
                None
            }
            event => Some(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;

    use super::*;
    use crate::{
        client::{PublishTrackOptions, Session as ClientSession},
        modules::test_support::{connected_client_sessions, within},
        object::ExtensionHeaders,
    };

    const ROOM: &[&str] = &["room"];
    const ALICE: &[&str] = &["room", "alice"];
    const TRACK: &str = "action";

    /// Registers `track` in `namespace` and writes one group into it, before
    /// anyone subscribed.
    async fn track_with_one_group(
        publisher: &ClientSession,
        namespace: &[&str],
        track: &str,
        payload: &'static [u8],
    ) {
        let writer = publisher
            .publish_track(namespace, track, PublishTrackOptions::default())
            .await
            .unwrap();
        writer
            .write(
                Bytes::from_static(payload),
                true,
                ExtensionHeaders::default(),
            )
            .await
            .unwrap();
    }

    /// Sends the PUBLISH of `track` in `namespace`; `false` once it is
    /// rejected.
    async fn publish(publisher: &ClientSession, namespace: &[&str], track: &str) -> bool {
        publisher
            .origin()
            .publish(publisher.moqt_session(), &namespace.join("/"), track)
            .await
            .is_ok()
    }

    async fn next_track(watch: &TrackWatch) -> PublishRequest {
        loop {
            match within(watch.next()).await {
                Some(WatchEvent::Track(request)) => return request,
                Some(WatchEvent::Namespace(_)) => {}
                None => panic!("the watch ended"),
            }
        }
    }

    async fn next_namespace(watch: &TrackWatch) -> Option<NamespaceEvent> {
        match within(watch.next()).await {
            Some(WatchEvent::Namespace(event)) => Some(event),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_published_track_is_read_from_the_group_sent_before_its_publish() {
        // Arrange
        let (session, publisher) = connected_client_sessions("watch-tracks-current").await;
        let watch = within(session.watch_tracks(ROOM)).await.unwrap();
        track_with_one_group(&publisher, ALICE, TRACK, b"go").await;

        // Act
        let (published, request) = tokio::join!(publish(&publisher, ALICE, TRACK), async {
            let request = next_track(&watch).await;
            let names = (request.namespace().to_vec(), request.track().to_string());
            let options = GroupStartOptions {
                priority: 0,
                delivery_timeout: Some(Duration::from_millis(300)),
                ..GroupStartOptions::default()
            };
            (names, request.accept_current_group(options).await.unwrap())
        });
        let ((namespace, track), subscription) = request;
        let object = within(subscription.next()).await.unwrap();

        // Assert
        assert!(published);
        assert_eq!(
            (namespace, track),
            (
                vec!["room".to_string(), "alice".to_string()],
                TRACK.to_string()
            )
        );
        assert_eq!(
            object.map(|object| object.payload),
            Some(Bytes::from_static(b"go"))
        );
    }

    #[tokio::test]
    async fn a_namespace_under_the_prefix_is_reported_when_published_and_withdrawn() {
        // Arrange
        let (session, publisher) = connected_client_sessions("watch-tracks-namespace").await;
        let watch = within(session.watch_tracks(ROOM)).await.unwrap();

        // Act
        let announcement = within(publisher.announce(ALICE)).await.unwrap();
        let published = next_namespace(&watch).await;
        within(announcement.close()).await.unwrap();
        let withdrawn = next_namespace(&watch).await;

        // Assert
        let alice = vec!["room".to_string(), "alice".to_string()];
        assert_eq!(
            (published, withdrawn),
            (
                Some(NamespaceEvent {
                    namespace: alice.clone(),
                    active: true
                }),
                Some(NamespaceEvent {
                    namespace: alice,
                    active: false
                })
            )
        );
    }

    #[tokio::test]
    async fn a_rejected_publish_fails_at_the_publisher() {
        // Arrange
        let (session, publisher) = connected_client_sessions("watch-tracks-reject").await;
        let watch = within(session.watch_tracks(ROOM)).await.unwrap();
        track_with_one_group(&publisher, ALICE, TRACK, b"go").await;

        // Act
        let (published, ()) = tokio::join!(publish(&publisher, ALICE, TRACK), async {
            next_track(&watch).await.reject().await.unwrap();
        });

        // Assert
        assert!(!published);
    }

    #[tokio::test]
    async fn a_publish_outside_the_prefix_is_not_handed_over() {
        // Arrange
        let (session, publisher) = connected_client_sessions("watch-tracks-outside").await;
        let watch = within(session.watch_tracks(ALICE)).await.unwrap();
        track_with_one_group(&publisher, &["room", "bob"], TRACK, b"bob").await;
        track_with_one_group(&publisher, ALICE, TRACK, b"alice").await;

        // Act
        let outside = within(publish(&publisher, &["room", "bob"], TRACK)).await;
        let (_, handed_over) = tokio::join!(publish(&publisher, ALICE, TRACK), async {
            let request = next_track(&watch).await;
            let namespace = request.namespace().to_vec();
            request.reject().await.unwrap();
            namespace
        });

        // Assert: the session rejected the PUBLISH under no watch itself
        assert!(!outside);
        assert_eq!(handed_over, ["room", "alice"]);
    }
}
