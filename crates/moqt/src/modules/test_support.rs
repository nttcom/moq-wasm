use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bytes::Bytes;
use tracing_subscriber::EnvFilter;

use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;
use crate::modules::transport::transport_connection::BoxedConnection;
use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;

use crate::{
    ClientConfig, DUAL, Endpoint, RequestFailure, Session, SessionEvent, Subscription, TrackReader,
    TransportProtocol,
    fetch::{Fetch, FetchDataReceiver, FetchDataSender, FetchOption},
    modules::moqt::{
        data_plane::object::{
            datagram_field::{DatagramField, ObjectDatagramPayload},
            object_datagram::ObjectDatagram,
        },
        domains::{connection::IncomingObjectNotification, session_context::SessionContext},
        runtime::dispatch::incoming_object::{IncomingDatagram, IncomingObject},
        version::{APPLICATION_PROTOCOLS, DRAFT18_PROTOCOLS},
    },
    modules::transport::application_protocol::ApplicationProtocols,
    object::{
        DataReceiver, DatagramReceiver, ExtensionHeaders, ForwardingPreference, KeyValuePair,
        Location, Object, ObjectStatus, StreamDataReceiver, StreamDataReceiverFactory,
        StreamDataSenderFactory, Subgroup, SubgroupObjectSender, SubgroupStart, VariantType,
    },
    publish::PublishOption,
    server::Handshake,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
    subscribe::{FilterType, SubscribeOption, SubscribeUpdateHandler},
    wire::{SubgroupId, SubgroupObject},
};

pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLISHER_PRIORITY: u8 = 128;
/// Well below quinn's 30 s default idle timeout and the 10 s control response
/// timeout, so a peer that is only detected as gone through either timeout
/// fails the wait.
pub(crate) const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn init_test_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _already_initialized = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_test_writer()
        .try_init();
}

fn unique_cert_dir(name: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("moqt-test-{name}-{}-{id}", std::process::id()))
}

/// A server config on a free port with a fresh self-signed certificate for
/// `localhost` and `127.0.0.1`.
pub(crate) fn self_signed_server_config(name: &str) -> ServerConfig {
    init_test_tracing();
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&unique_cert_dir(name), &["localhost", "127.0.0.1"]).unwrap();
    ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
    }
}

pub(crate) fn server_transport_creator<C: TransportConnectionCreator>(name: &str) -> C {
    let config = self_signed_server_config(name);
    C::server(
        &config.cert_path,
        &config.key_path,
        0,
        config.keep_alive_interval_sec,
        APPLICATION_PROTOCOLS,
    )
    .unwrap()
}

/// More than quinn's default limit of 100 concurrent bidirectional streams.
const OPEN_STREAMS: usize = 150;

/// Opens [`OPEN_STREAMS`] bidirectional streams and keeps every one open.
pub(crate) async fn open_bi_streams(
    connection: &BoxedConnection,
) -> anyhow::Result<Vec<impl Sized>> {
    let mut streams = Vec::with_capacity(OPEN_STREAMS);
    for _ in 0..OPEN_STREAMS {
        streams.push(connection.open_bi().await?);
    }
    Ok(streams)
}

/// Starts a DUAL server on a free port whose accept loop resolves the first
/// incoming handshake (CLIENT_SETUP received, SERVER_SETUP not yet sent);
/// returns the port and the accept task.
pub(crate) fn spawn_dual_server_handshake(name: &str) -> (u16, tokio::task::JoinHandle<Handshake>) {
    spawn_dual_server_handshake_offering(name, APPLICATION_PROTOCOLS)
}

fn spawn_dual_server_handshake_offering(
    name: &str,
    protocols: ApplicationProtocols,
) -> (u16, tokio::task::JoinHandle<Handshake>) {
    let server_config = self_signed_server_config(name);
    let server = Endpoint::<DUAL>::create_server_offering(&server_config, protocols).unwrap();
    let port = server.local_port().unwrap();
    let accept = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
    (port, accept)
}

/// Starts a DUAL server on a free port whose accept loop resolves the first
/// incoming session; returns the port and the accept task.
pub(crate) fn spawn_dual_server(name: &str) -> (u16, tokio::task::JoinHandle<Session>) {
    let (port, handshake) = spawn_dual_server_handshake(name);
    let accept = tokio::spawn(async move { handshake.await.unwrap().accept().await.unwrap() });
    (port, accept)
}

/// `spawn_dual_server`, offering only draft-18.
pub(crate) fn spawn_draft18_server(name: &str) -> (u16, tokio::task::JoinHandle<Session>) {
    let (port, handshake) = spawn_dual_server_handshake_offering(name, DRAFT18_PROTOCOLS);
    let accept = tokio::spawn(async move { handshake.await.unwrap().accept().await.unwrap() });
    (port, accept)
}

pub(crate) fn dual_client_with_config(config: ClientConfig) -> Endpoint<DUAL> {
    Endpoint::<DUAL>::create_client(&config).unwrap()
}

/// `dual_client_with_config`, offering only draft-18.
pub(crate) fn draft18_client_with_config(config: ClientConfig) -> Endpoint<DUAL> {
    Endpoint::<DUAL>::create_client_offering(&config, DRAFT18_PROTOCOLS).unwrap()
}

pub(crate) fn insecure_client_config() -> ClientConfig {
    ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: None,
        follow_go_away: false,
    }
}

pub(crate) fn dual_client() -> Endpoint<DUAL> {
    dual_client_with_config(insecure_client_config())
}

/// Connects a DUAL client to the server started by `spawn_dual_server` and
/// returns both established sessions as (client, server).
pub(crate) async fn connect_sessions(
    url: &str,
    accept: tokio::task::JoinHandle<Session>,
) -> anyhow::Result<(Session, Session)> {
    connect_sessions_from(&dual_client(), url, accept).await
}

pub(crate) async fn connect_sessions_from<T: TransportProtocol>(
    client: &Endpoint<T>,
    url: &str,
    accept: tokio::task::JoinHandle<Session>,
) -> anyhow::Result<(Session, Session)> {
    let client = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        client.connect(url).await?.await
    })
    .await??;
    let server = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept).await??;
    Ok((client, server))
}

pub(crate) async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, future)
        .await
        .expect("the exchange timed out")
}

pub(crate) async fn receive_disconnected(session: &Session) -> anyhow::Result<()> {
    tokio::time::timeout(DISCONNECT_TIMEOUT, async {
        while !matches!(session.receive_event().await?, SessionEvent::Disconnected()) {}
        anyhow::Ok(())
    })
    .await?
}

pub(crate) async fn spawn_connected_dual_sessions(name: &str) -> (Session, Session) {
    let (port, accept) = spawn_dual_server(name);
    connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
        .await
        .unwrap()
}

/// Connects a DUAL client and server that offer only draft-18, over raw
/// QUIC for `moqt` or WebTransport for `https`.
pub(crate) async fn spawn_connected_draft18_sessions(
    name: &str,
    scheme: &str,
) -> (Session, Session) {
    let (port, accept) = spawn_draft18_server(name);
    let client = draft18_client_with_config(insecure_client_config());
    let path = if scheme == "https" { "/moq" } else { "" };
    connect_sessions_from(
        &client,
        &format!("{scheme}://127.0.0.1:{port}{path}"),
        accept,
    )
    .await
    .unwrap()
}

/// Answers the client's PUBLISH with PUBLISH_OK and registers the object
/// receiver for its track alias.
pub(crate) async fn accept_publish(server: &Session) -> Subscription {
    let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
        panic!("expected PUBLISH from the client");
    };
    let subscription = handler.ok(128, FilterType::LargestObject).await.unwrap();
    handler.accept_data_receiver().await;
    subscription
}

pub(crate) struct PublishedTrack {
    pub(crate) client: Session,
    pub(crate) server: Session,
    pub(crate) published: Subscription,
    pub(crate) accepted: Subscription,
}

pub(crate) async fn published_track(name: &str) -> PublishedTrack {
    let (client, server) = spawn_connected_dual_sessions(name).await;
    published_track_between(client, server).await
}

/// The client publishes `ns/track` and the server accepts it.
pub(crate) async fn published_track_between(client: Session, server: Session) -> PublishedTrack {
    let publisher = client.publisher();
    let (published, accepted) = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        tokio::join!(
            publisher.publish("ns".into(), "track".into(), PublishOption::default()),
            accept_publish(&server)
        )
    })
    .await
    .unwrap();
    PublishedTrack {
        client,
        server,
        published: published.unwrap(),
        accepted,
    }
}

/// A subscription the client made to the server's `ns/track`.
pub(crate) struct SubscribedTrack {
    pub(crate) client: Session,
    pub(crate) server: Session,
    pub(crate) subscription: Subscription,
}

/// The client subscribes to `ns/track` and the server accepts it.
pub(crate) async fn subscribed_track(name: &str) -> SubscribedTrack {
    let (client, server) = spawn_connected_dual_sessions(name).await;
    let mut subscriber = client.subscriber();
    let (subscribed, ()) = within(async {
        tokio::join!(
            subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
            async {
                let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
                    panic!("expected SUBSCRIBE from the client");
                };
                handler
                    .ok(0, crate::object::ContentExists::False)
                    .await
                    .unwrap();
            }
        )
    })
    .await;
    SubscribedTrack {
        client,
        server,
        subscription: subscribed.unwrap(),
    }
}

/// Has the server update the Forward State of the track it accepted and
/// returns the update as the publishing client receives it.
pub(crate) async fn received_subscribe_update(track: &PublishedTrack) -> SubscribeUpdateHandler {
    track
        .server
        .subscriber()
        .update_forward(&track.accepted, true)
        .await
        .unwrap();
    let event = within(track.client.receive_event()).await.unwrap();
    let SessionEvent::SubscribeUpdate(update) = event else {
        panic!("expected SUBSCRIBE_UPDATE, got {event:?}");
    };
    update
}

pub(crate) fn object_with_extensions() -> Object {
    Object {
        location: Location {
            group_id: 16_384,
            object_id: 16_385,
        },
        forwarding: ForwardingPreference::Subgroup { subgroup_id: 3 },
        publisher_priority: 7,
        status: ObjectStatus::Normal,
        extension_headers: ExtensionHeaders::new(vec![
            KeyValuePair {
                key: 0x10,
                value: VariantType::Even(16_384),
            },
            KeyValuePair {
                key: 0x21,
                value: VariantType::Odd(Bytes::from_static(b"\x01\x00\x80")),
            },
            KeyValuePair {
                key: 0x0d,
                value: VariantType::Odd(Bytes::from_static(b"\x00\xff\x7f")),
            },
            KeyValuePair {
                key: 0x21,
                value: VariantType::Odd(Bytes::from_static(b"\x02\xff")),
            },
        ]),
        payload: Bytes::from_static(b"\x00object\xff"),
        ends_group: false,
    }
}

pub(crate) async fn open_subgroup(
    factory: &StreamDataSenderFactory,
    group_id: u64,
    subgroup_id: SubgroupId,
) -> SubgroupObjectSender {
    factory
        .next()
        .await
        .unwrap()
        .start_raw(group_id, subgroup_id, PUBLISHER_PRIORITY)
        .await
        .unwrap()
}

pub(crate) async fn send_payload(sender: &mut SubgroupObjectSender, payload: &'static [u8]) {
    sender
        .send_raw(0, SubgroupObject::new_payload(Bytes::from_static(payload)))
        .await
        .unwrap();
}

pub(crate) async fn send_subgroup_object(
    session: &Session,
    subscription: &Subscription,
    object: &Object,
) {
    let ForwardingPreference::Subgroup { subgroup_id } = object.forwarding else {
        panic!("expected an object forwarded on a subgroup stream");
    };
    let mut sender = session
        .publisher()
        .create_stream(subscription)
        .next()
        .await
        .unwrap()
        .start(SubgroupStart {
            group_id: object.location.group_id,
            subgroup_id,
            publisher_priority: object.publisher_priority,
            ends_group: false,
        })
        .await
        .unwrap();
    sender.send_object(object).await.unwrap();
    sender.close().await.unwrap();
}

pub(crate) async fn read_subgroup_objects(receiver: &mut StreamDataReceiver) -> Vec<Object> {
    let mut objects = Vec::new();
    while let Some(received) = receiver.receive().await.unwrap() {
        if let Subgroup::Object(object) = received {
            objects.push(object);
        }
    }
    objects
}

/// The first subgroup stream of `subscription`; resolves once a header has
/// arrived.
pub(crate) async fn first_subgroup_stream(
    server: &Session,
    subscription: &Subscription,
) -> crate::object::StreamDataReceiver {
    subgroup_receiver(server, subscription)
        .await
        .next()
        .await
        .unwrap()
}

/// Resolves once the first subgroup header has arrived; call after sending it.
pub(crate) async fn subgroup_receiver(
    server: &Session,
    subscription: &Subscription,
) -> StreamDataReceiverFactory {
    let DataReceiver::Stream(factory) = server
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .unwrap()
    else {
        panic!("expected a subgroup stream receiver");
    };
    factory
}

/// The data receiver resolves only once the first object has arrived, so
/// this must run after the publisher has sent something.
pub(crate) async fn datagram_receiver(
    server: &Session,
    subscription: &Subscription,
) -> DatagramReceiver {
    let DataReceiver::Datagram(receiver) = server
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .unwrap()
    else {
        panic!("expected a datagram receiver");
    };
    receiver
}

/// The data receiver resolves only once the first object has arrived, so
/// this must run after the publisher has sent something.
pub(crate) async fn subscribed_track_reader(
    server: &Session,
    subscription: &Subscription,
) -> TrackReader {
    TrackReader::new(subgroup_receiver(server, subscription).await)
}

pub(crate) async fn read_fetch_objects(
    receiver: &mut FetchDataReceiver,
) -> Vec<crate::object::Object> {
    let mut objects = Vec::new();
    loop {
        match receiver.receive().await.unwrap() {
            Fetch::Object(object) => objects.push(object),
            Fetch::UnknownRange { last } => panic!("unexpected unknown range through {last:?}"),
            Fetch::End => return objects,
        }
    }
}

/// The object `object_datagram` carries, for tests that send it.
pub(crate) fn datagram_object() -> crate::object::Object {
    object_datagram(0).into_object()
}

pub(crate) fn object_datagram(track_alias: u64) -> ObjectDatagram {
    ObjectDatagram::new(
        track_alias,
        0,
        DatagramField {
            object_id: Some(0),
            publisher_priority: PUBLISHER_PRIORITY,
            extension_headers: None,
            end_of_group: false,
            payload: ObjectDatagramPayload::Payload(Bytes::from_static(b"object")),
        },
    )
}

pub(crate) async fn notify_datagram(
    context: &SessionContext,
    track_alias: u64,
) -> IncomingObjectNotification {
    context
        .connection()
        .notify_incoming_object(
            track_alias,
            IncomingObject::Datagram(IncomingDatagram::new(datagram_object())),
        )
        .await
}

pub(crate) async fn register_and_take_data_receiver(
    context: &SessionContext,
    request_id: u64,
    track_alias: u64,
) -> tokio::sync::mpsc::UnboundedReceiver<IncomingObject> {
    context
        .register_data_receiver(&context.connection(), request_id, track_alias)
        .await
        .unwrap();
    context
        .receiver_map
        .lock()
        .await
        .remove(&track_alias)
        .unwrap()
}

/// A subscriber-initiated subscription with the given ids, for operations
/// that only address it.
pub(crate) fn subscription(request_id: u64, track_alias: u64) -> Subscription {
    Subscription::SubscriberInitiated(crate::SubscriberInitiatedSubscription {
        request_id,
        track_namespace: "ns".to_string(),
        track_name: "track".to_string(),
        track_alias,
        expires: 0,
        group_order: crate::object::GroupOrder::Ascending,
        content_exists: crate::object::ContentExists::False,
        subscriber_priority: 128,
        filter_type: crate::subscribe::FilterType::LargestObject,
        delivery_timeout: None,
        max_cache_duration: None,
    })
}

pub(crate) struct Rejection {
    pub(crate) code: RequestErrorCode,
    pub(crate) reason: String,
}

pub(crate) fn rejection<T, E: Into<anyhow::Error>>(result: Result<T, E>) -> Rejection {
    let Err(error) = result else {
        panic!("expected the request to fail");
    };
    let error: anyhow::Error = error.into();
    match error.downcast_ref::<RequestFailure>() {
        Some(RequestFailure::Rejected { code, reason, .. }) => Rejection {
            code: *code,
            reason: reason.clone(),
        },
        _ => panic!("expected a rejection, got {error:#}"),
    }
}

pub(crate) fn location(group_id: u64, object_id: u64) -> crate::object::Location {
    crate::object::Location {
        group_id,
        object_id,
    }
}

pub(crate) struct FetchedStream {
    _client: Session,
    _server: Session,
    pub(crate) sender: FetchDataSender,
    pub(crate) receiver: FetchDataReceiver,
}

pub(crate) async fn fetched_stream(name: &str) -> FetchedStream {
    let (client, server) = spawn_connected_dual_sessions(name).await;
    let mut subscriber = client.subscriber();
    // Matches the EndOfTrack location in the FETCH round-trip test.
    let end = location(16_385, 0);
    let (handle, sender) = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        tokio::join!(
            subscriber.fetch(
                "ns".into(),
                "track".into(),
                location(0, 0),
                end,
                FetchOption::default(),
            ),
            async {
                let SessionEvent::Fetch(handler) = server.receive_event().await.unwrap() else {
                    panic!("expected FETCH from the client");
                };
                handler.ok(true, end).await.unwrap();
                server
                    .publisher()
                    .create_fetch_stream(handler.request_id)
                    .await
                    .unwrap()
            }
        )
    })
    .await
    .unwrap();
    let receiver = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        subscriber.accept_fetch_receiver(&handle.unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    FetchedStream {
        _client: client,
        _server: server,
        sender,
        receiver,
    }
}
