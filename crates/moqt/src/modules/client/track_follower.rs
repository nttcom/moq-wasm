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
    // On wasm32 the PUBLISH handler is !Send, but wasm32 runs on one thread.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
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
    Published(Box<PublishHandler>),
    Object(ReceivedObject),
    Left,
    Ended,
}

impl Follow {
    async fn run(mut self) {
        let mut follower = Follower::Idle;
        loop {
            let step = tokio::select! {
                announcement = self.announcements.recv() => {
                    announcement.map_or(Step::Ended, Step::Announced)
                }
                handler = self.publish_receiver.recv() => handler.map_or(Step::Ended, |handler| Step::Published(Box::new(handler))),
                object = next_object(follower.followed()) => object.map_or(Step::Left, Step::Object),
                () = left(follower.followed()) => Step::Left,
            };
            match step {
                Step::Announced(announcement) => {
                    if announcement.namespace == self.namespace
                        && follower.announced(announcement.active)
                    {
                        follower.follow(self.subscribe().await);
                    }
                }
                Step::Published(handler) => {
                    if follower.followed().is_some() {
                        let _ = handler
                            .error(
                                RequestErrorCode::Uninterested,
                                "another publisher of the track is followed".to_string(),
                            )
                            .await;
                    } else {
                        follower.follow(self.accept(*handler).await);
                    }
                }
                Step::Object(object) => {
                    if self.event_sender.send(FollowEvent::Object(object)).is_err() {
                        return;
                    }
                }
                Step::Left => {
                    let handover = follower.left();
                    if self.event_sender.send(FollowEvent::PublisherLeft).is_err() {
                        return;
                    }
                    if handover {
                        follower.follow(self.subscribe().await);
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

async fn next_object(followed: Option<&Subscription>) -> Option<ReceivedObject> {
    match followed {
        Some(subscription) => subscription.next().await.ok().flatten(),
        None => pending().await,
    }
}

/// Resolves on the followed subscription's PUBLISH_DONE.
async fn left(followed: Option<&Subscription>) {
    match followed {
        Some(subscription) => {
            subscription.done().await;
        }
        None => pending().await,
    }
}

/// The follow task's state between steps (`Follower` in
/// `docs/architecture/moqt/models/track_follower.qnt`); a SUBSCRIBE or
/// PUBLISH_OK in flight is awaited from `Idle`.
///
/// A publisher that announces while another is followed takes over at the
/// followed one's PUBLISH_DONE: a relay may still hold a publisher that has
/// gone, and that PUBLISH_DONE then arrives after the new PUBLISH_NAMESPACE.
enum Follower<S = Subscription> {
    Idle,
    Following(S),
    /// The namespace was announced while `S` was followed.
    FollowingWithHandover(S),
}

impl<S> Follower<S> {
    fn followed(&self) -> Option<&S> {
        match self {
            Self::Idle => None,
            Self::Following(followed) | Self::FollowingWithHandover(followed) => Some(followed),
        }
    }

    /// Takes a PUBLISH_NAMESPACE (`active`) or PUBLISH_NAMESPACE_DONE of the
    /// followed namespace; returns whether to subscribe now. A
    /// PUBLISH_NAMESPACE_DONE drops a pending handover: the peer reports the
    /// namespace as a whole, so no publisher is left to take over.
    fn announced(&mut self, active: bool) -> bool {
        match std::mem::replace(self, Self::Idle) {
            Self::Idle => active,
            Self::Following(followed) | Self::FollowingWithHandover(followed) => {
                *self = if active {
                    Self::FollowingWithHandover(followed)
                } else {
                    Self::Following(followed)
                };
                false
            }
        }
    }

    /// Takes the followed subscription's end; returns whether to subscribe
    /// now, for a publisher that announced while the one that left was
    /// followed.
    fn left(&mut self) -> bool {
        matches!(
            std::mem::replace(self, Self::Idle),
            Self::FollowingWithHandover(_)
        )
    }

    /// Follows the answer to a SUBSCRIBE or PUBLISH_OK sent from `Idle`, if
    /// any.
    fn follow(&mut self, followed: Option<S>) {
        *self = followed.map_or(Self::Idle, Self::Following);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use tokio::sync::Notify;

    use super::*;
    use crate::{
        client::{PublishTrackOptions, Session as ClientSession, TrackWriter},
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

    struct RecordSubscribes(mpsc::UnboundedSender<String>);

    #[async_trait]
    impl EventHandler for RecordSubscribes {
        async fn handle(&self, event: SessionEvent) -> Option<SessionEvent> {
            if let SessionEvent::Subscribe(handler) = &event {
                let _ = self.0.send(handler.track_namespace.clone());
            }
            Some(event)
        }
    }

    fn record_subscribes(publisher: &ClientSession) -> mpsc::UnboundedReceiver<String> {
        let (sender, receiver) = mpsc::unbounded_channel();
        publisher.add_handler(Arc::new(RecordSubscribes(sender)));
        receiver
    }

    async fn write(writer: &TrackWriter, payload: &'static [u8], new_group: bool) {
        writer
            .write(
                Bytes::from_static(payload),
                new_group,
                ExtensionHeaders::default(),
            )
            .await
            .unwrap();
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
        let mut follower = Follower::<()>::Idle;

        // Act / Assert
        assert!(follower.announced(true));
    }

    #[test]
    fn a_publisher_announcing_while_another_is_followed_takes_over_when_that_one_leaves() {
        // Arrange
        let mut follower = Follower::Following(());

        // Act
        let subscribed_at_announce = follower.announced(true);
        let subscribed_at_leave = follower.left();

        // Assert
        assert!(!subscribed_at_announce);
        assert!(subscribed_at_leave);
    }

    #[test]
    fn a_publisher_that_withdraws_while_another_is_followed_is_not_subscribed_when_that_one_leaves()
    {
        // Arrange
        let mut follower = Follower::Following(());
        assert!(!follower.announced(true));

        // Act
        let subscribed_at_withdrawal = follower.announced(false);
        let subscribed_at_leave = follower.left();

        // Assert
        assert!(!subscribed_at_withdrawal);
        assert!(!subscribed_at_leave);
    }

    #[test]
    fn the_followed_publisher_leaving_without_a_waiting_publisher_subscribes_nothing() {
        // Arrange
        let mut follower = Follower::Idle;
        assert!(follower.announced(true));
        follower.follow(Some(()));

        // Act / Assert
        assert!(!follower.left());
    }

    #[tokio::test]
    async fn publishes_for_other_tracks_or_namespaces_are_not_taken() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-other-publish").await;
        let publish = PublishTrackOptions {
            publish: true,
            ..PublishTrackOptions::default()
        };

        // Act
        let other_track = within(publisher.publish_track(NAMESPACE, "other", publish)).await;
        let other_namespace =
            within(publisher.publish_track(&["room", "other"], TRACK, publish)).await;
        let target = within(publisher.publish_track(NAMESPACE, TRACK, publish))
            .await
            .unwrap();
        write(&target, b"keys", true).await;
        let event = within(follower.next()).await;

        // Assert: non-matching PUBLISHes are rejected instead of accepted by the follower
        assert!(other_track.is_err());
        assert!(other_namespace.is_err());
        assert_eq!(payload(event), Some(Bytes::from_static(b"keys")));
    }

    #[tokio::test]
    async fn unmatching_announcements_do_not_subscribe() {
        // Arrange
        let (_session, follower, publisher) =
            follower_and_publisher("follow-unmatching-announce").await;
        let mut subscribes = record_subscribes(&publisher);
        let announcement = within(publisher.announce(NAMESPACE)).await.unwrap();
        let initial_subscribe = within(subscribes.recv()).await.unwrap();

        // Act
        within(announcement.close()).await.unwrap();
        let _child = within(publisher.announce(&["room", "viewer", "child"]))
            .await
            .unwrap();
        write_when_subscribed(&publisher, b"keys").await;
        let event = within(follower.next()).await;
        let final_subscribe = within(subscribes.recv()).await.unwrap();

        // Assert
        assert_eq!(payload(event), Some(Bytes::from_static(b"keys")));
        assert_eq!(
            [initial_subscribe, final_subscribe],
            [NAMESPACE.join("/"), NAMESPACE.join("/")]
        );
        assert!(subscribes.try_recv().is_err());
    }

    #[tokio::test]
    async fn an_announcement_while_following_takes_over_only_after_the_followed_publisher_leaves() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-handover").await;
        let mut subscribes = record_subscribes(&publisher);
        let writer = publisher
            .publish_track(NAMESPACE, TRACK, PublishTrackOptions::default())
            .await
            .unwrap();
        let _first = within(publisher.announce(NAMESPACE)).await.unwrap();
        assert!(within(writer.next_subscriber()).await);
        within(subscribes.recv()).await.unwrap();
        write(&writer, b"first", true).await;
        assert_eq!(
            payload(within(follower.next()).await),
            Some(Bytes::from_static(b"first"))
        );

        // Act: the track is registered again as soon as it ends, for the second announcement
        let _second = within(publisher.announce(NAMESPACE)).await.unwrap();
        let early_subscribe =
            tokio::time::timeout(Duration::from_millis(200), subscribes.recv()).await;
        within(publisher.origin().end_track(&NAMESPACE.join("/"), TRACK))
            .await
            .unwrap();
        let writer = publisher
            .publish_track(NAMESPACE, TRACK, PublishTrackOptions::default())
            .await
            .unwrap();
        let left = within(follower.next()).await;
        let handover_subscribe = within(subscribes.recv()).await;
        assert!(within(writer.next_subscriber()).await);
        write(&writer, b"second", true).await;
        let next_object = within(follower.next()).await;

        // Assert
        assert!(early_subscribe.is_err());
        assert_eq!(left, Some(FollowEvent::PublisherLeft));
        assert_eq!(handover_subscribe, Some(NAMESPACE.join("/")));
        assert_eq!(payload(next_object), Some(Bytes::from_static(b"second")));
    }

    #[tokio::test]
    async fn an_announced_track_is_subscribed_from_the_next_group() {
        // Arrange
        let (_session, follower, publisher) = follower_and_publisher("follow-next-group").await;
        let writer = publisher
            .publish_track(NAMESPACE, TRACK, PublishTrackOptions::default())
            .await
            .unwrap();
        write(&writer, b"group-0-object-0", true).await;

        // Act
        let _announcement = within(publisher.announce(NAMESPACE)).await.unwrap();
        assert!(within(writer.next_subscriber()).await);
        write(&writer, b"group-0-object-1", false).await;
        write(&writer, b"group-1-object-0", true).await;
        let event = within(follower.next()).await;

        // Assert
        assert_eq!(
            payload(event),
            Some(Bytes::from_static(b"group-1-object-0"))
        );
    }

    mod properties {
        use proptest::{collection::vec, prelude::*};

        use super::super::Follower;

        /// `Follower` of `track_follower_current` in track_follower.qnt.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum Model {
            Idle,
            Subscribing,
            Following,
            FollowingWithHandover,
        }

        #[derive(Debug, Clone)]
        enum Event {
            /// `takeAnnouncement` of a PUBLISH_NAMESPACE (`true`) or
            /// PUBLISH_NAMESPACE_DONE of the followed namespace.
            Announced(bool),
            /// `subscribeAnswered`: SUBSCRIBE_OK (`true`) or an error.
            SubscribeAnswered(bool),
            /// `takePublish`, accepted (`true`) or failed; refused while
            /// following.
            Published(bool),
            /// `followedLeft`.
            Left,
        }

        fn event() -> impl Strategy<Value = Event> {
            prop_oneof![
                any::<bool>().prop_map(Event::Announced),
                any::<bool>().prop_map(Event::SubscribeAnswered),
                any::<bool>().prop_map(Event::Published),
                Just(Event::Left),
            ]
        }

        /// The model's next state, or `None` where the action is disabled.
        fn model_step(model: Model, event: &Event) -> Option<Model> {
            match (model, event) {
                (Model::Subscribing, Event::SubscribeAnswered(true)) => Some(Model::Following),
                (Model::Subscribing, Event::SubscribeAnswered(false)) => Some(Model::Idle),
                (Model::Subscribing, _) | (_, Event::SubscribeAnswered(_)) => None,
                (Model::Idle, Event::Announced(true)) => Some(Model::Subscribing),
                (_, Event::Announced(true)) => Some(Model::FollowingWithHandover),
                (Model::Idle, Event::Announced(false)) => Some(Model::Idle),
                (_, Event::Announced(false)) => Some(Model::Following),
                (Model::Idle, Event::Published(true)) => Some(Model::Following),
                (_, Event::Published(_)) => Some(model),
                (Model::Idle, Event::Left) => None,
                (Model::Following, Event::Left) => Some(Model::Idle),
                (Model::FollowingWithHandover, Event::Left) => Some(Model::Subscribing),
            }
        }

        /// `subscribing`: the follow task awaits a SUBSCRIBE answer.
        fn abstraction(follower: &Follower<()>, subscribing: bool) -> Model {
            match follower {
                Follower::Idle if subscribing => Model::Subscribing,
                Follower::Idle => Model::Idle,
                Follower::Following(()) => Model::Following,
                Follower::FollowingWithHandover(()) => Model::FollowingWithHandover,
            }
        }

        proptest! {
            #[test]
            fn follower_steps_like_the_model(events in vec(event(), 0..32)) {
                // Arrange
                let mut follower = Follower::Idle;
                let mut subscribing = false;
                let mut model = Model::Idle;

                for event in events {
                    let Some(next) = model_step(model, &event) else {
                        continue;
                    };

                    // Act
                    match event {
                        Event::Announced(active) => subscribing = follower.announced(active),
                        Event::SubscribeAnswered(ok) => {
                            subscribing = false;
                            follower.follow(ok.then_some(()));
                        }
                        Event::Published(ok) => {
                            if follower.followed().is_none() {
                                follower.follow(ok.then_some(()));
                            }
                        }
                        Event::Left => subscribing = follower.left(),
                    }
                    model = next;

                    // Assert
                    prop_assert_eq!(abstraction(&follower, subscribing), model);
                }
            }
        }
    }
}
