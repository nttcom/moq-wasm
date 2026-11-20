use std::{future::pending, sync::Arc};

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};

use crate::{
    RequestErrorCode, Session, SessionEvent,
    modules::{
        client::{
            event_handler::{EventHandler, HandlerId},
            routes::Routes,
            subscription::{Reading, ReceivedObject, Subscription},
        },
        executor::{self, JoinHandle},
    },
    namespace::NamespaceEvent,
    publish::PublishHandler,
    subscribe::{FilterType, SubscribeOption},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowEvent {
    Object(ReceivedObject),
    /// The followed publisher ended its subscription. State set by its
    /// objects is stale, and the next publisher numbers its groups anew.
    PublisherLeft,
}

/// A track that peers publish one after another, such as the input track of
/// a browser viewer: each publisher in turn is followed from the start of
/// its next group, whether it announces the namespace (PUBLISH_NAMESPACE,
/// answered with SUBSCRIBE) or publishes the track (PUBLISH). While one is
/// followed, a PUBLISH of another is refused, and one that announced takes
/// over once the followed one leaves.
pub struct TrackFollower {
    routes: Arc<Routes>,
    handler: HandlerId,
    events: Mutex<mpsc::UnboundedReceiver<FollowEvent>>,
    _task: FollowTask,
}

impl TrackFollower {
    pub(super) fn new(
        session: Arc<Session>,
        routes: Arc<Routes>,
        namespace: Vec<String>,
        track: String,
        announcements: mpsc::UnboundedReceiver<NamespaceEvent>,
    ) -> Self {
        let (publish_sender, publish_receiver) = mpsc::unbounded_channel();
        let handler = routes.add_handler(Arc::new(PublishTaker {
            namespace: namespace.join("/"),
            track: track.clone(),
            publish_sender,
        }));
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        let task = FollowTask::run(Follow {
            session,
            routes: routes.clone(),
            namespace,
            track,
            announcements,
            publish_receiver,
            event_sender,
        });
        Self {
            routes,
            handler,
            events: Mutex::new(event_receiver),
            _task: task,
        }
    }

    /// The next object of the followed publisher, or the end of its
    /// subscription; `None` once the session ends.
    pub async fn next(&self) -> Option<FollowEvent> {
        self.events.lock().await.recv().await
    }
}

impl Drop for TrackFollower {
    fn drop(&mut self) {
        self.routes.remove_handler(self.handler);
    }
}

struct PublishTaker {
    namespace: String,
    track: String,
    publish_sender: mpsc::UnboundedSender<PublishHandler>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl EventHandler for PublishTaker {
    async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
        match event {
            SessionEvent::Publish(handler)
                if handler.track_namespace == self.namespace
                    && handler.track_name == self.track =>
            {
                let _ = self.publish_sender.send(handler);
                None
            }
            event => Some(event),
        }
    }
}

struct FollowTask {
    handle: JoinHandle,
}

impl FollowTask {
    fn run(follow: Follow) -> Self {
        let handle = executor::spawn("moqt-client-track-follower", follow.run());
        Self { handle }
    }
}

impl Drop for FollowTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct Follow {
    session: Arc<Session>,
    routes: Arc<Routes>,
    namespace: Vec<String>,
    track: String,
    announcements: mpsc::UnboundedReceiver<NamespaceEvent>,
    publish_receiver: mpsc::UnboundedReceiver<PublishHandler>,
    event_sender: mpsc::UnboundedSender<FollowEvent>,
}

enum Step {
    Announced(NamespaceEvent),
    Published(PublishHandler),
    Object(ReceivedObject),
    Left,
    Ended,
}

impl Follow {
    async fn run(mut self) {
        let mut followed: Option<Subscription> = None;
        let mut handover = PublisherHandover::default();
        loop {
            let step = tokio::select! {
                announcement = self.announcements.recv() => {
                    announcement.map_or(Step::Ended, Step::Announced)
                }
                handler = self.publish_receiver.recv() => handler.map_or(Step::Ended, Step::Published),
                object = next_object(&followed) => object.map_or(Step::Left, Step::Object),
                () = left(&followed) => Step::Left,
            };
            match step {
                Step::Announced(announcement) => {
                    if announcement.active
                        && announcement.namespace == self.namespace
                        && handover.announced(followed.is_some())
                    {
                        followed = self.subscribe().await;
                    }
                }
                Step::Published(handler) => {
                    if followed.is_some() {
                        let _ = handler
                            .error(
                                RequestErrorCode::Uninterested,
                                "another publisher of the track is followed".to_string(),
                            )
                            .await;
                    } else {
                        followed = self.accept(handler).await;
                    }
                }
                Step::Object(object) => {
                    if self.event_sender.send(FollowEvent::Object(object)).is_err() {
                        return;
                    }
                }
                Step::Left => {
                    followed = None;
                    if self.event_sender.send(FollowEvent::PublisherLeft).is_err() {
                        return;
                    }
                    if handover.followed_publisher_left() {
                        followed = self.subscribe().await;
                    }
                }
                Step::Ended => return,
            }
        }
    }

    /// `None` is not fatal: the publisher may have left before answering, and
    /// the next announcement subscribes again.
    async fn subscribe(&self) -> Option<Subscription> {
        let namespace = self.namespace.join("/");
        let option = SubscribeOption {
            filter_type: FilterType::NextGroupStart,
            ..SubscribeOption::default()
        };
        match self
            .session
            .subscriber()
            .subscribe(namespace.clone(), self.track.clone(), option)
            .await
        {
            Ok(subscription) => Some(self.subscription(subscription)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    track_namespace = %namespace,
                    track_name = %self.track,
                    "announced track not subscribed"
                );
                None
            }
        }
    }

    async fn accept(&self, handler: PublishHandler) -> Option<Subscription> {
        let priority = SubscribeOption::default().subscriber_priority;
        match handler
            .ok(priority, FilterType::NextGroupStart, handler.forward)
            .await
        {
            Ok(subscription) => {
                handler.accept_data_receiver().await;
                Some(self.subscription(subscription))
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    track_namespace = %handler.track_namespace,
                    track_name = %handler.track_name,
                    "failed to accept PUBLISH"
                );
                None
            }
        }
    }

    fn subscription(&self, subscription: crate::Subscription) -> Subscription {
        Subscription::new(
            self.session.clone(),
            self.routes.clone(),
            subscription,
            Reading::Unstarted,
        )
    }
}

async fn next_object(followed: &Option<Subscription>) -> Option<ReceivedObject> {
    match followed {
        Some(subscription) => subscription.next().await.ok().flatten(),
        None => pending().await,
    }
}

/// Resolves on the followed subscription's PUBLISH_DONE.
async fn left(followed: &Option<Subscription>) {
    match followed {
        Some(subscription) => {
            subscription.done().await;
        }
        None => pending().await,
    }
}

/// A publisher that announces while another is followed takes over at the
/// followed one's PUBLISH_DONE: a relay may still hold a publisher that has
/// gone, and that PUBLISH_DONE then arrives after the new PUBLISH_NAMESPACE.
#[derive(Default)]
struct PublisherHandover {
    announced_while_following: bool,
}

impl PublisherHandover {
    /// Returns whether to subscribe now.
    fn announced(&mut self, following: bool) -> bool {
        self.announced_while_following = following;
        !following
    }

    /// Returns whether to subscribe now, for a publisher that announced
    /// while the one that left was followed.
    fn followed_publisher_left(&mut self) -> bool {
        std::mem::take(&mut self.announced_while_following)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        client::{PublishTrackOptions, Session as ClientSession},
        modules::test_support::{connected_client_sessions, within},
        object::ExtensionHeaders,
    };

    const NAMESPACE: &[&str] = &["room", "viewer"];
    const TRACK: &str = "input";

    async fn follower_and_publisher(name: &str) -> (ClientSession, TrackFollower, ClientSession) {
        let (follower_session, publisher) = connected_client_sessions(name).await;
        let follower = within(follower_session.follow_track(NAMESPACE, TRACK))
            .await
            .unwrap();
        (follower_session, follower, publisher)
    }

    async fn write_when_subscribed(publisher: &ClientSession, payload: &'static [u8]) {
        let writer = publisher
            .publish_track(NAMESPACE, TRACK, PublishTrackOptions::default())
            .await
            .unwrap();
        let _announcement = within(publisher.announce(NAMESPACE)).await.unwrap();
        assert!(within(writer.next_subscriber()).await);
        writer
            .write(
                Bytes::from_static(payload),
                true,
                ExtensionHeaders::default(),
            )
            .await
            .unwrap();
    }

    #[derive(Default)]
    struct RejectSubscribes(Notify);

    #[async_trait]
    impl EventHandler for RejectSubscribes {
        async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
            let SessionEvent::Subscribe(handler) = event else {
                return Some(event);
            };
            let _ = handler
                .error(RequestErrorCode::TrackDoesNotExist, "gone".to_string())
                .await;
            self.0.notify_one();
            None
        }
    }

    fn payload(event: Option<FollowEvent>) -> Option<Bytes> {
        match event {
            Some(FollowEvent::Object(object)) => Some(object.payload),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_publisher_that_announces_the_namespace_is_followed() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-announced").await;

        // Act
        write_when_subscribed(&publisher, b"keys").await;
        let event = within(follower.next()).await;

        // Assert
        assert_eq!(payload(event), Some(Bytes::from_static(b"keys")));
    }

    #[tokio::test]
    async fn a_publisher_that_publishes_the_track_is_followed() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-published").await;

        // Act
        let writer = within(publisher.publish_track(
            NAMESPACE,
            TRACK,
            PublishTrackOptions {
                publish: true,
                ..PublishTrackOptions::default()
            },
        ))
        .await
        .unwrap();
        writer
            .write(
                Bytes::from_static(b"keys"),
                true,
                ExtensionHeaders::default(),
            )
            .await
            .unwrap();
        let event = within(follower.next()).await;

        // Assert
        assert_eq!(payload(event), Some(Bytes::from_static(b"keys")));
    }

    #[tokio::test]
    async fn the_followed_publisher_ending_its_track_is_reported() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-left").await;
        write_when_subscribed(&publisher, b"keys").await;
        within(follower.next()).await;

        // Act
        within(publisher.origin().end_track(&NAMESPACE.join("/"), TRACK))
            .await
            .unwrap();
        let event = within(follower.next()).await;

        // Assert
        assert_eq!(event, Some(FollowEvent::PublisherLeft));
    }

    #[tokio::test]
    async fn a_rejected_subscribe_leaves_the_follower_waiting_for_a_publish() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-rejected").await;
        let rejecting = Arc::new(RejectSubscribes::default());
        let handler = publisher.add_handler(rejecting.clone());
        let _announcement = within(publisher.announce(NAMESPACE)).await.unwrap();
        within(rejecting.0.notified()).await;
        publisher.remove_handler(handler);

        // Act
        let writer = within(publisher.publish_track(
            NAMESPACE,
            TRACK,
            PublishTrackOptions {
                publish: true,
                ..PublishTrackOptions::default()
            },
        ))
        .await
        .unwrap();
        writer
            .write(
                Bytes::from_static(b"keys"),
                true,
                ExtensionHeaders::default(),
            )
            .await
            .unwrap();
        let event = within(follower.next()).await;

        // Assert
        assert_eq!(payload(event), Some(Bytes::from_static(b"keys")));
    }

    #[tokio::test]
    async fn the_follower_ends_with_the_session() {
        // Arrange
        let (session, follower, _publisher) = follower_and_publisher("follow-closed").await;

        // Act
        within(session.close()).await;
        let event = within(follower.next()).await;

        // Assert
        assert_eq!(event, None);
    }

    #[test]
    fn a_publisher_announcing_while_none_is_followed_is_subscribed_at_once() {
        // Arrange
        let mut handover = PublisherHandover::default();

        // Act / Assert
        assert!(handover.announced(false));
    }

    #[test]
    fn a_publisher_announcing_while_another_is_followed_takes_over_when_that_one_leaves() {
        // Arrange
        let mut handover = PublisherHandover::default();

        // Act
        let subscribed_at_announce = handover.announced(true);
        let subscribed_at_leave = handover.followed_publisher_left();

        // Assert
        assert!(!subscribed_at_announce);
        assert!(subscribed_at_leave);
    }

    #[test]
    fn the_followed_publisher_leaving_without_a_waiting_publisher_subscribes_nothing() {
        // Arrange
        let mut handover = PublisherHandover::default();
        handover.announced(false);

        // Act / Assert
        assert!(!handover.followed_publisher_left());
    }
}
