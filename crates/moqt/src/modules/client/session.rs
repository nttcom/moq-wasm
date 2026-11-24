use std::{sync::Arc, time::Duration};

use crate::{
    ClientConfig, Endpoint, Session as MoqtSession, Subscription as MoqtSubscription,
    TransferredBytes, TransportClose,
    fetch::FetchOption,
    modules::{
        client::{
            announcement::Announcement,
            event_handler::{EventHandler, HandlerId},
            event_loop_task::EventLoopTask,
            fetch::Fetch,
            join_namespace,
            namespace_watch::NamespaceWatch,
            request_error::{FetchError, RequestError},
            routes::Routes,
            subscription::{Reading, Subscription},
            track_follower::TrackFollower,
            track_watch::TrackWatch,
            track_writer::TrackWriter,
        },
        executor,
        moqt::domains::track_writer::DEFAULT_PRIORITY,
    },
    namespace::NamespaceInterest,
    object::{GroupOrder, Location},
    publish::{Origin, Retention},
    subscribe::{FilterType, SubscribeOption},
    wire::RequestKind,
};

#[cfg(not(target_arch = "wasm32"))]
type ClientTransport = crate::NATIVE;
#[cfg(target_arch = "wasm32")]
type ClientTransport = crate::BROWSER;

/// Matches the relay's default RELAY_CACHE_TTL_SECS, so a FETCH the relay
/// forwards for a range it never cached can still be answered here.
const DEFAULT_RETENTION: Duration = Duration::from_secs(60);

/// How `Session::subscribe` subscribes (draft-14 §9.7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubscribeOptions {
    pub filter: FilterType,
    pub forward: bool,
    /// The Subscriber Priority (draft-14 §7.2): the relay sends a lower value
    /// first.
    pub priority: u8,
    /// The DELIVERY TIMEOUT (draft-14 §9.2.1.2): the relay stops sending an
    /// object this long after receiving it. `None` has every object sent.
    pub delivery_timeout: Option<Duration>,
    pub group_order: GroupOrder,
}

impl Default for SubscribeOptions {
    fn default() -> Self {
        let defaults = SubscribeOption::default();
        Self {
            filter: defaults.filter_type,
            forward: defaults.forward,
            priority: defaults.subscriber_priority,
            delivery_timeout: None,
            group_order: defaults.group_order,
        }
    }
}

/// How `Session::subscribe_group_start` and `Session::subscribe_current_group`
/// subscribe; their filter is Largest Object and their Forward State 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupStartOptions {
    pub priority: u8,
    pub delivery_timeout: Option<Duration>,
    pub group_order: GroupOrder,
}

impl Default for GroupStartOptions {
    fn default() -> Self {
        let defaults = SubscribeOptions::default();
        Self {
            priority: defaults.priority,
            delivery_timeout: defaults.delivery_timeout,
            group_order: defaults.group_order,
        }
    }
}

impl GroupStartOptions {
    pub(super) fn subscribe_option(self) -> SubscribeOption {
        SubscribeOption {
            subscriber_priority: self.priority,
            delivery_timeout: self
                .delivery_timeout
                .map(|timeout| timeout.as_millis() as u64),
            group_order: self.group_order,
            ..SubscribeOption::default()
        }
    }
}

/// How `Session::publish_track` registers a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishTrackOptions {
    /// How long the cache keeps objects to answer FETCH; 60 s by default.
    pub retention: Retention,
    /// Also sends the track with PUBLISH (draft-14 §9.13); announce the
    /// namespace first.
    pub publish: bool,
    /// Sends each object in its own datagram (draft-14 §10.3) instead of on
    /// subgroup streams.
    pub datagram: bool,
    /// The Publisher Priority (draft-14 §7.1), 128 by default: among the
    /// session's tracks a lower value is sent first.
    pub priority: u8,
}

impl Default for PublishTrackOptions {
    fn default() -> Self {
        Self {
            retention: Retention::For(DEFAULT_RETENTION),
            publish: false,
            datagram: false,
            priority: DEFAULT_PRIORITY,
        }
    }
}

/// A client session. It reads
/// every event of the `moqt::Session` it wraps, and stops reading once it is
/// dropped or closed. GOAWAY and reconnection stay with `moqt::Session`.
pub struct Session {
    session: Arc<MoqtSession>,
    routes: Arc<Routes>,
    event_loop: EventLoopTask,
}

impl Session {
    /// Takes over `session`'s events. Natively, call it inside a tokio
    /// runtime: it spawns the event loop there.
    // On wasm32 the browser session is !Send, but wasm32 runs on one thread.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub fn new(session: MoqtSession) -> Self {
        let session = Arc::new(session);
        let routes = Arc::new(Routes::default());
        let event_loop = EventLoopTask::run(session.clone(), routes.clone());
        Self {
            session,
            routes,
            event_loop,
        }
    }

    /// Connects to `url` (`moqt://` for raw QUIC, `https://` for
    /// WebTransport) on an endpoint of its own; in a browser, over
    /// WebTransport with the WebSocket fallback.
    pub async fn connect(url: &str, config: &ClientConfig) -> anyhow::Result<Self> {
        let endpoint = Endpoint::<ClientTransport>::create_client(config)?;
        let session = endpoint.connect(url).await?.await?;
        Ok(Self::new(session))
    }

    /// The wrapped session, for what this type does not cover; its events
    /// are not for the caller.
    pub fn moqt_session(&self) -> &Arc<MoqtSession> {
        &self.session
    }

    /// The tracks `publish_track` registered, which a handler may write into
    /// as well.
    pub fn origin(&self) -> &Origin {
        &self.routes.origin
    }

    /// Hands the session's events to `handler` before the `Origin`, after
    /// the handlers added earlier.
    pub fn add_handler(&self, handler: Arc<dyn EventHandler>) -> HandlerId {
        self.routes.add_handler(handler)
    }

    pub fn remove_handler(&self, id: HandlerId) {
        self.routes.remove_handler(id);
    }

    /// SUBSCRIBE (draft-14 §9.7).
    pub async fn subscribe(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
        options: SubscribeOptions,
    ) -> Result<Subscription, RequestError> {
        let option = SubscribeOption {
            filter_type: options.filter,
            forward: options.forward,
            ..GroupStartOptions {
                priority: options.priority,
                delivery_timeout: options.delivery_timeout,
                group_order: options.group_order,
            }
            .subscribe_option()
        };
        let subscription = self
            .session
            .subscriber()
            .subscribe(join_namespace(namespace), track.to_string(), option)
            .await
            .map_err(RequestError::of(RequestKind::Subscribe))?;
        Ok(self.subscription(subscription, Reading::Unstarted))
    }

    /// Subscribes and reads the group the largest object is in from its
    /// first object, through a Joining FETCH (draft-14 §9.16.2).
    pub async fn subscribe_current_group(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
        options: GroupStartOptions,
    ) -> Result<Subscription, RequestError> {
        let (subscription, reader) = self
            .session
            .subscriber()
            .subscribe_current_group(
                join_namespace(namespace),
                track.to_string(),
                options.subscribe_option(),
            )
            .await
            .map_err(RequestError::of(RequestKind::Subscribe))?;
        Ok(self.subscription(subscription, Reading::Objects(reader)))
    }

    /// Subscribe from group start (draft-18 §5.1.3.1): joins the current group via Joining `FETCH`,
    /// or sends `NEW_GROUP_REQUEST` (§10.2.13) when a dynamic-group track's largest object ID is >= `new_group_after`.
    pub async fn subscribe_group_start(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
        new_group_after: u64,
        options: GroupStartOptions,
    ) -> Result<Subscription, RequestError> {
        let (subscription, reader) = self
            .session
            .subscriber()
            .subscribe_group_start(
                join_namespace(namespace),
                track.to_string(),
                new_group_after,
                options.subscribe_option(),
            )
            .await
            .map_err(RequestError::of(RequestKind::Subscribe))?;
        Ok(self.subscription(subscription, Reading::Objects(reader)))
    }

    fn subscription(&self, subscription: MoqtSubscription, reading: Reading) -> Subscription {
        Subscription::new(
            self.session.clone(),
            self.routes.clone(),
            subscription,
            reading,
        )
    }

    /// A Standalone FETCH (draft-14 §9.16.1). `end`'s object is the last
    /// object plus 1, and 0 means the whole group. Returns once the response
    /// stream has arrived.
    pub async fn fetch(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
        start: Location,
        end: Location,
    ) -> Result<Fetch, FetchError> {
        let mut subscriber = self.session.subscriber();
        let handle = subscriber
            .fetch(
                join_namespace(namespace),
                track.to_string(),
                start,
                end,
                FetchOption::default(),
            )
            .await
            .map_err(RequestError::of(RequestKind::Fetch))?;
        let receiver = subscriber
            .accept_fetch_receiver(&handle)
            .await
            .map_err(FetchError::Stream)?;
        Ok(Fetch::new(receiver))
    }

    /// TRACK_STATUS (draft-14 §9.20): the track's largest location, or
    /// `None` while it has none.
    pub async fn track_status(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
    ) -> Result<Option<Location>, RequestError> {
        let status = self
            .session
            .subscriber()
            .track_status(join_namespace(namespace), track.to_string(), Vec::new())
            .await
            .map_err(RequestError::of(RequestKind::TrackStatus))?;
        Ok(status.content_exists.largest())
    }

    /// `SUBSCRIBE_NAMESPACE` (draft-14 §9.28) for namespaces under `prefix`.
    /// Fails on rejection (e.g. `NAMESPACE_PREFIX_OVERLAP` with another `watch_namespaces`, `watch_tracks`, or `follow_track` on this session).
    pub async fn watch_namespaces(
        &self,
        prefix: &[impl AsRef<str>],
    ) -> Result<NamespaceWatch, RequestError> {
        let elements = prefix
            .iter()
            .map(|element| element.as_ref().to_string())
            .collect();
        // Register before sending the request so immediate peer announcements aren't missed.
        let events = self.routes.watchers.watch(elements);
        let prefix = join_namespace(prefix);
        self.session
            .subscriber()
            .subscribe_namespace(prefix.clone(), NamespaceInterest::Namespaces)
            .await
            .map_err(RequestError::of(RequestKind::SubscribeNamespace))?;
        Ok(NamespaceWatch::new(self.session.clone(), prefix, events))
    }

    /// `SUBSCRIBE_NAMESPACE` (draft-14 §9.28) for namespaces and tracks under `prefix`: reports each namespace and hands over each matching `PUBLISH`, which nothing else on the session then sees.
    /// Fails on rejection (e.g. `NAMESPACE_PREFIX_OVERLAP` with another `watch_namespaces`, `watch_tracks`, or `follow_track` on this session).
    pub async fn watch_tracks(
        &self,
        prefix: &[impl AsRef<str>],
    ) -> Result<TrackWatch, RequestError> {
        let elements = prefix
            .iter()
            .map(|element| element.as_ref().to_string())
            .collect();
        // Watched before the request: the peer publishes the namespaces and
        // tracks it knows right away.
        let watch = TrackWatch::new(self.session.clone(), self.routes.clone(), elements);
        self.session
            .subscriber()
            .subscribe_namespace(
                join_namespace(prefix),
                NamespaceInterest::NamespacesAndTracks,
            )
            .await
            .map_err(RequestError::of(RequestKind::SubscribeNamespace))?;
        Ok(watch)
    }

    /// Follows `track` in `namespace` as peers publish it one after another,
    /// through a SUBSCRIBE_NAMESPACE (draft-14 §9.28) for the namespace and
    /// its tracks; see `TrackFollower`. Fails when the SUBSCRIBE_NAMESPACE
    /// is rejected, as one whose namespace overlaps another
    /// `watch_namespaces`, `watch_tracks` or `follow_track` of this session
    /// is (NAMESPACE_PREFIX_OVERLAP): one session follows one track per
    /// namespace.
    pub async fn follow_track(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
    ) -> Result<TrackFollower, RequestError> {
        let elements: Vec<String> = namespace
            .iter()
            .map(|element| element.as_ref().to_string())
            .collect();
        // Watch before sending SUBSCRIBE_NAMESPACE: the peer sends the
        // namespaces it already knows right away.
        let announcements = self.routes.watchers.watch(elements.clone());
        let follower = TrackFollower::new(
            self.session.clone(),
            self.routes.clone(),
            elements,
            track.to_string(),
            announcements,
        );
        self.session
            .subscriber()
            .subscribe_namespace(
                join_namespace(namespace),
                NamespaceInterest::NamespacesAndTracks,
            )
            .await
            .map_err(RequestError::of(RequestKind::SubscribeNamespace))?;
        Ok(follower)
    }

    /// PUBLISH_NAMESPACE (draft-14 §9.23).
    pub async fn announce(
        &self,
        namespace: &[impl AsRef<str>],
    ) -> Result<Announcement, RequestError> {
        let namespace = join_namespace(namespace);
        self.session
            .publisher()
            .publish_namespace(namespace.clone())
            .await
            .map_err(RequestError::of(RequestKind::PublishNamespace))?;
        Ok(Announcement::new(self.session.clone(), namespace))
    }

    /// Registers a track to publish, whose group ids start at the wall-clock
    /// time in µs, so a later run of the same publisher does not reuse a
    /// location the relay still caches. A track registered again keeps its
    /// objects, so once it has one, registering it with the other `datagram`
    /// fails with `PublishTrackError::OtherDelivery` and changes nothing.
    /// Also fails when the PUBLISH fails.
    pub async fn publish_track(
        &self,
        namespace: &[impl AsRef<str>],
        track: &str,
        options: PublishTrackOptions,
    ) -> anyhow::Result<TrackWriter> {
        let namespace = join_namespace(namespace);
        let origin = &self.routes.origin;
        origin
            .ensure_track(
                &namespace,
                track,
                executor::system_time().as_micros() as u64,
                options.retention,
            )
            .await;
        origin
            .set_delivery(&namespace, track, options.datagram)
            .await?;
        origin
            .set_publisher_priority(&namespace, track, options.priority)
            .await?;
        if options.publish {
            origin.publish(&self.session, &namespace, track).await?;
        }
        Ok(TrackWriter::new(
            self.routes.clone(),
            namespace,
            track.to_string(),
        ))
    }

    pub fn transferred_bytes(&self) -> TransferredBytes {
        self.session.transferred_bytes()
    }

    /// Resolves once the session's last connection has closed.
    pub async fn closed(&self) -> TransportClose {
        self.session.closed().await
    }

    /// Stops answering the peer, ends every namespace watch, `done` and
    /// `next_subscriber`, and closes the session with NO_ERROR, as
    /// `moqt::Session::close` does.
    pub async fn close(&self) {
        self.event_loop.stop();
        self.routes.end();
        self.session.close().await;
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;
    use crate::{
        RequestErrorCode, TerminationErrorCode,
        client::PublishTrackError,
        modules::{
            client::subscription::ReceivedObject,
            test_support::{
                TEST_VERSIONS, connected_client_sessions, data_receiver, insecure_client_config,
                next_datagram, next_subgroup, object_with_extensions, spawn_native_server, within,
            },
        },
        namespace::NamespaceEvent,
        object::{DataReceiver, ExtensionHeaders, ForwardingPreference, Subgroup},
    };

    const NAMESPACE: &[&str] = &["room", "alice"];

    const DATAGRAM: PublishTrackOptions = PublishTrackOptions {
        retention: Retention::For(DEFAULT_RETENTION),
        publish: false,
        datagram: true,
        priority: DEFAULT_PRIORITY,
    };

    /// Subscribes below `client::Session`, so the test sees how each object
    /// arrives.
    async fn data_of(client: &Session, track: &str) -> DataReceiver {
        let session = client.moqt_session();
        let option = SubscribeOption {
            filter_type: FilterType::NextGroupStart,
            ..SubscribeOption::default()
        };
        let subscription = within(session.subscriber().subscribe(
            join_namespace(NAMESPACE),
            track.to_string(),
            option,
        ))
        .await
        .unwrap();
        data_receiver(session, &subscription).await
    }

    async fn write_new_group(writer: &TrackWriter, payload: &'static [u8]) {
        writer
            .write(
                Bytes::from_static(payload),
                true,
                ExtensionHeaders::default(),
            )
            .await
            .unwrap();
    }

    fn assert_send_sync<T: Send + Sync>(_: &T) {}

    fn assert_send<T: Send>(_: T) {}

    #[tokio::test]
    async fn the_session_and_what_it_returns_can_be_used_from_other_threads() {
        // Arrange
        let (client, server) = connected_client_sessions("client-send-sync").await;
        let writer = server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();
        let subscription = within(client.subscribe(NAMESPACE, "chat", SubscribeOptions::default()))
            .await
            .unwrap();

        // Act / Assert: compiles only when the types and futures are Send (+ Sync), and `done` outlives its subscription
        assert_send_sync(&client);
        assert_send_sync(&writer);
        assert_send_sync(&subscription);
        let namespace = vec!["room".to_string()];
        assert_send(client.subscribe(&namespace, "chat", SubscribeOptions::default()));
        assert_send(client.watch_namespaces(&namespace));
        assert_send(client.publish_track(&namespace, "t", PublishTrackOptions::default()));
        assert_send(subscription.next());
        assert_send(writer.write(Bytes::new(), true, ExtensionHeaders::default()));
        let done = subscription.done();
        drop(subscription);
        assert_send(done);
    }

    #[tokio::test]
    async fn a_datagram_track_published_again_after_it_ended_still_sends_datagrams() {
        // Arrange
        let (client, server) = connected_client_sessions("client-datagram-again").await;
        let writer = server
            .publish_track(NAMESPACE, "cursor", DATAGRAM)
            .await
            .unwrap();
        write_new_group(&writer, b"a").await;
        within(
            server
                .origin()
                .end_track(&join_namespace(NAMESPACE), "cursor"),
        )
        .await
        .unwrap();

        // Act
        let writer = server
            .publish_track(NAMESPACE, "cursor", DATAGRAM)
            .await
            .unwrap();
        let mut receiver = data_of(&client, "cursor").await;
        write_new_group(&writer, b"b").await;
        let object = within(next_datagram(&mut receiver)).await;

        // Assert
        assert_eq!(object.forwarding, ForwardingPreference::Datagram);
        assert_eq!(object.payload, Bytes::from_static(b"b"));
    }

    #[tokio::test]
    async fn a_datagram_track_published_again_on_streams_is_refused() {
        // Arrange
        let (_client, server) = connected_client_sessions("client-datagram-to-streams").await;
        let writer = server
            .publish_track(NAMESPACE, "cursor", DATAGRAM)
            .await
            .unwrap();
        write_new_group(&writer, b"a").await;
        within(
            server
                .origin()
                .end_track(&join_namespace(NAMESPACE), "cursor"),
        )
        .await
        .unwrap();

        // Act
        let refused = server
            .publish_track(NAMESPACE, "cursor", PublishTrackOptions::default())
            .await;

        // Assert
        let Err(error) = refused else {
            panic!("expected the registration to be refused");
        };
        assert!(
            matches!(
                error.downcast_ref::<PublishTrackError>(),
                Some(PublishTrackError::OtherDelivery { datagram: true, .. })
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_registration_leaves_the_publisher_priority_unchanged() {
        // Arrange
        let (client, server) = connected_client_sessions("client-refused-priority").await;
        let writer = server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();
        write_new_group(&writer, b"a").await;

        // Act
        let refused = server
            .publish_track(
                NAMESPACE,
                "chat",
                PublishTrackOptions {
                    priority: 7,
                    ..DATAGRAM
                },
            )
            .await;
        let mut receiver = data_of(&client, "chat").await;
        write_new_group(&writer, b"b").await;
        let mut stream = within(next_subgroup(&mut receiver)).await;
        let header = within(stream.receive()).await.unwrap();

        // Assert
        assert!(refused.is_err());
        let Some(Subgroup::Header(header)) = header else {
            panic!("expected a subgroup header, got {header:?}");
        };
        assert_eq!(header.publisher_priority, DEFAULT_PRIORITY);
    }

    #[tokio::test]
    async fn connect_opens_a_session_that_closes_with_no_error() {
        // Arrange
        let (port, accept) = spawn_native_server("client-connect");

        // Act
        let session = within(Session::connect(
            &format!("moqt://127.0.0.1:{port}"),
            &ClientConfig {
                versions: TEST_VERSIONS.to_vec(),
                ..insecure_client_config()
            },
        ))
        .await
        .unwrap();
        let server = within(accept).await.unwrap();
        within(session.close()).await;

        // Assert
        assert_eq!(within(server.closed()).await.code, Some(0));
    }

    #[tokio::test]
    async fn a_subscription_reads_what_the_peer_s_track_writer_writes() {
        // Arrange
        let (client, server) = connected_client_sessions("client-subscribe").await;
        let writer = server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();
        let subscription = within(client.subscribe(NAMESPACE, "chat", SubscribeOptions::default()))
            .await
            .unwrap();
        let extension_headers = object_with_extensions().extension_headers;

        // Act
        let group_id = writer
            .write(Bytes::from_static(b"o0"), true, extension_headers.clone())
            .await
            .unwrap();
        let object = within(subscription.next()).await.unwrap().unwrap();

        // Assert
        assert_eq!(
            object,
            ReceivedObject {
                group_id: group_id.unwrap(),
                subgroup_id: 0,
                object_id: 0,
                status: None,
                payload: Bytes::from_static(b"o0"),
                extension_headers,
            }
        );
        assert_eq!(subscription.largest(), None);
    }

    #[tokio::test]
    async fn a_group_written_just_before_the_session_closes_reaches_the_peer() {
        // Arrange: larger than the stream's flow control window, so most of it is
        // still unsent when `write_group` returns
        let (client, server) = connected_client_sessions("client-close-after-write").await;
        let writer = server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();
        let subscription = within(client.subscribe(NAMESPACE, "chat", SubscribeOptions::default()))
            .await
            .unwrap();
        let payload = Bytes::from(vec![7u8; 8 * 1024 * 1024]);
        let reading = tokio::spawn(async move { subscription.next().await });

        // Act
        within(writer.write_group(payload.clone())).await.unwrap();
        within(server.close()).await;
        let object = within(reading).await.unwrap().unwrap().unwrap();

        // Assert
        assert_eq!(object.payload.len(), payload.len());
    }

    #[tokio::test]
    async fn a_subscription_read_by_group_yields_each_subgroup_stream() {
        // Arrange
        let (client, server) = connected_client_sessions("client-groups").await;
        let writer = server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();
        let subscription = within(client.subscribe(NAMESPACE, "chat", SubscribeOptions::default()))
            .await
            .unwrap();

        // Act
        let group_id = writer
            .write_group(Bytes::from_static(b"catalog"))
            .await
            .unwrap();
        let group = within(subscription.next_group()).await.unwrap().unwrap();
        let object = within(group.next()).await.unwrap().unwrap();

        // Assert
        assert_eq!(Some(group.group_id()), group_id);
        assert_eq!(object.payload, Bytes::from_static(b"catalog"));
        assert!(subscription.next().await.is_err());
    }

    #[tokio::test]
    async fn a_fetch_returns_the_objects_of_the_group_then_ends() {
        // Arrange
        let (client, server) = connected_client_sessions("client-fetch").await;
        let writer = server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();
        for (new_group, payload) in [(true, b"o0"), (false, b"o1")] {
            writer
                .write(
                    Bytes::from_static(payload),
                    new_group,
                    ExtensionHeaders::default(),
                )
                .await
                .unwrap();
        }
        let largest = within(client.track_status(NAMESPACE, "chat"))
            .await
            .unwrap()
            .unwrap();

        // Act
        let fetch = within(client.fetch(
            NAMESPACE,
            "chat",
            Location {
                group_id: largest.group_id,
                object_id: 0,
            },
            Location {
                group_id: largest.group_id,
                object_id: 0,
            },
        ))
        .await
        .unwrap();
        let mut payloads = Vec::new();
        while let Some(object) = within(fetch.next()).await.unwrap() {
            payloads.push(object.payload);
        }

        // Assert
        assert_eq!(largest.object_id, 1);
        assert_eq!(
            payloads,
            [Bytes::from_static(b"o0"), Bytes::from_static(b"o1")]
        );
    }

    #[tokio::test]
    async fn a_fetch_the_peer_rejects_fails_with_its_request_error() {
        // Arrange
        let (client, _server) = connected_client_sessions("client-fetch-rejected").await;
        let location = Location {
            group_id: 0,
            object_id: 0,
        };

        // Act
        let failed = within(client.fetch(NAMESPACE, "missing", location, location)).await;

        // Assert
        let Err(FetchError::Request(rejected)) = failed else {
            panic!("expected the FETCH to be rejected");
        };
        assert_eq!(rejected.kind, RequestKind::Fetch);
        assert_ne!(rejected.code(), RequestErrorCode::InternalError);
    }

    #[tokio::test]
    async fn track_status_of_a_track_without_objects_is_none() {
        // Arrange
        let (client, server) = connected_client_sessions("client-track-status").await;
        server
            .publish_track(NAMESPACE, "chat", PublishTrackOptions::default())
            .await
            .unwrap();

        // Act
        let largest = within(client.track_status(NAMESPACE, "chat")).await;

        // Assert
        assert_eq!(largest.unwrap(), None);
    }

    #[tokio::test]
    async fn a_rejected_request_carries_the_draft_error_code() {
        // Arrange
        let (client, _server) = connected_client_sessions("client-request-error").await;

        // Act
        let rejected = within(client.subscribe(NAMESPACE, "missing", SubscribeOptions::default()))
            .await
            .err()
            .unwrap();

        // Assert
        assert_eq!(rejected.kind, RequestKind::Subscribe);
        assert_eq!(rejected.code(), RequestErrorCode::TrackDoesNotExist);
        assert_eq!(rejected.reason(), "missing is not published here");
    }

    #[tokio::test]
    async fn a_watch_reports_a_namespace_announced_and_withdrawn_under_its_prefix() {
        // Arrange
        let (client, server) = connected_client_sessions("client-watch").await;
        let watch = within(client.watch_namespaces(&["room"])).await.unwrap();

        // Act
        let announcement = within(server.announce(NAMESPACE)).await.unwrap();
        let published = within(watch.next()).await;
        within(announcement.close()).await.unwrap();
        let withdrawn = within(watch.next()).await;

        // Assert
        let namespace: Vec<String> = NAMESPACE.iter().map(|e| e.to_string()).collect();
        assert_eq!(
            [published, withdrawn],
            [
                Some(NamespaceEvent {
                    namespace: namespace.clone(),
                    active: true,
                }),
                Some(NamespaceEvent {
                    namespace,
                    active: false,
                }),
            ]
        );
    }

    #[tokio::test]
    async fn closing_a_namespace_watch_ends_the_read_that_waits() {
        // Arrange
        let (client, _server) = connected_client_sessions("client-watch-close").await;
        let watch = within(client.watch_namespaces(&["room"])).await.unwrap();

        // Act
        let (event, closed) = within(async { tokio::join!(watch.next(), watch.close()) }).await;

        // Assert
        closed.unwrap();
        assert_eq!(event, None);
        assert_eq!(watch.next().await, None);
    }

    #[tokio::test]
    async fn closing_the_wrapped_session_with_an_error_ends_the_namespace_watches() {
        // Arrange
        let (client, server) = connected_client_sessions("client-close-with-error").await;
        let watch = within(client.watch_namespaces(&["room"])).await.unwrap();

        // Act
        client
            .moqt_session()
            .close_with_error(TerminationErrorCode::NoError, "closed by the application");
        let close = within(server.closed()).await;

        // Assert
        assert_eq!(close.code, Some(0));
        assert_eq!(close.reason, "closed by the application");
        assert_eq!(within(watch.next()).await, None);
    }

    #[tokio::test]
    async fn closing_the_session_ends_its_namespace_watches() {
        // Arrange
        let (client, _server) = connected_client_sessions("client-close").await;
        let watch = within(client.watch_namespaces(&["room"])).await.unwrap();

        // Act
        within(client.close()).await;

        // Assert
        assert_eq!(within(watch.next()).await, None);
    }
}
