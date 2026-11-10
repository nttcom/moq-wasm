use std::{
    collections::HashMap,
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use tracing::{
    Event,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
    subscriber::DefaultGuard,
};
use tracing_subscriber::{
    EnvFilter, Layer,
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
};

use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;
use crate::modules::transport::{
    accept_error::AcceptError,
    application_protocol::ApplicationProtocols,
    connect_target::ConnectTarget,
    server_transport_config::MAX_CONCURRENT_BIDI_STREAMS,
    transport_connection::{BoxedConnection, BoxedReceiveStream, BoxedSendStream},
    transport_connection_creator::{TransportAccepting, TransportConnectionCreator},
    websocket::websocket_listener::WebSocketListener,
};
use crate::namespace::NamespaceInterest;

use crate::{
    CertificateVerification, ClientConfig, Endpoint, NATIVE, RequestFailure, Session, SessionEvent,
    Subscription, TrackReader, TransportProtocol,
    fetch::{Fetch, FetchDataReceiver, FetchDataSender, FetchOption},
    modules::moqt::{
        data_plane::object::{
            datagram_field::{DatagramField, ObjectDatagramPayload},
            object_datagram::ObjectDatagram,
        },
        domains::{connection::IncomingObjectNotification, session_context::SessionContext},
        runtime::dispatch::incoming_object::{
            IncomingDatagram, IncomingObject, IncomingObjectReceiver,
        },
        version::{Version, server_application_protocols},
    },
    object::{
        DataReceiver, ExtensionHeaders, ForwardingPreference, KeyValuePair, Location, Object,
        ObjectStatus, StreamDataReceiver, StreamDataSenderFactory, Subgroup, SubgroupObjectSender,
        SubgroupStart, TrackData, VariantType,
    },
    publish::PublishOption,
    server::Handshake,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
    subscribe::{FilterType, SubscribeOption, SubscribeUpdateHandler},
    wire::{SubgroupId, SubgroupObject},
};

/// Key 0x4000 is in draft-18's Mandatory Track Property range (§2.5.1).
pub(crate) fn mandatory_track_properties() -> crate::TrackProperties {
    crate::TrackProperties::from_pairs(vec![KeyValuePair {
        key: 0x4000,
        value: VariantType::Even(1),
    }])
}

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

/// Tests that name no draft write and read draft-14 bytes and messages.
pub(crate) const TEST_VERSIONS: &[Version] = &[Version::Draft14];

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
        versions: TEST_VERSIONS.to_vec(),
    }
}

pub(crate) fn server_transport_creator<C: TransportConnectionCreator>(name: &str) -> C {
    let config = self_signed_server_config(name);
    C::server(
        &config.cert_path,
        &config.key_path,
        0,
        config.keep_alive_interval_sec,
        server_application_protocols(&config.versions).unwrap(),
    )
    .unwrap()
}

/// More than quinn's default limit of 100 concurrent bidirectional streams.
const OPEN_STREAMS: usize = 150;

/// Opens [`OPEN_STREAMS`] bidirectional streams and keeps every one open.
async fn open_bi_streams(connection: &BoxedConnection) -> anyhow::Result<Vec<impl Sized>> {
    let mut streams = Vec::with_capacity(OPEN_STREAMS);
    for _ in 0..OPEN_STREAMS {
        streams.push(connection.open_bi().await?);
    }
    Ok(streams)
}

/// Asserts that both sides open [`OPEN_STREAMS`] bidirectional streams at
/// once within [`HANDSHAKE_TIMEOUT`].
pub(crate) async fn assert_both_sides_open_bi_streams(
    client: &BoxedConnection,
    server: &BoxedConnection,
) {
    let opened = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        tokio::join!(open_bi_streams(client), open_bi_streams(server))
    })
    .await;
    let (from_client, from_server) = opened.expect("every stream opens in time");
    assert!(from_client.is_ok(), "{:?}", from_client.err());
    assert!(from_server.is_ok(), "{:?}", from_server.err());
}

/// Starts a NATIVE server on a free port whose accept loop resolves the first
/// incoming handshake (CLIENT_SETUP received, SERVER_SETUP not yet sent);
/// returns the port and the accept task.
pub(crate) fn spawn_native_server_handshake(
    name: &str,
) -> (u16, tokio::task::JoinHandle<Handshake>) {
    spawn_native_server_handshake_offering(name, TEST_VERSIONS)
}

pub(crate) fn spawn_native_server_handshake_offering(
    name: &str,
    versions: &[Version],
) -> (u16, tokio::task::JoinHandle<Handshake>) {
    let server_config = ServerConfig {
        versions: versions.to_vec(),
        ..self_signed_server_config(name)
    };
    let server = Endpoint::<NATIVE>::create_server(&server_config).unwrap();
    let port = server.local_port().unwrap();
    let accept = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
    (port, accept)
}

/// Starts a NATIVE server on a free port whose accept loop resolves the first
/// incoming session; returns the port and the accept task.
pub(crate) fn spawn_native_server(name: &str) -> (u16, tokio::task::JoinHandle<Session>) {
    spawn_native_server_offering(name, TEST_VERSIONS)
}

/// Every draft. Iterating it prints each draft to the test's captured
/// output, so a failing test that loops over it names the draft it failed
/// over.
pub(crate) const DRAFTS: Drafts = Drafts;

pub(crate) struct Drafts;

impl IntoIterator for Drafts {
    type Item = Version;
    type IntoIter = std::iter::Inspect<std::array::IntoIter<Version, 2>, fn(&Version)>;

    fn into_iter(self) -> Self::IntoIter {
        let announce: fn(&Version) = |version| eprintln!("running over {version:?}");
        [Version::Draft14, Version::Draft18]
            .into_iter()
            .inspect(announce)
    }
}

/// `spawn_native_server`, accepting only `version`.
pub(crate) fn spawn_server(
    name: &str,
    version: Version,
) -> (u16, tokio::task::JoinHandle<Session>) {
    spawn_native_server_offering(name, &[version])
}

/// `spawn_native_server`, offering `versions`.
pub(crate) fn spawn_native_server_offering(
    name: &str,
    versions: &[Version],
) -> (u16, tokio::task::JoinHandle<Session>) {
    let (port, handshake) = spawn_native_server_handshake_offering(name, versions);
    let accept = tokio::spawn(async move {
        handshake
            .await
            .unwrap()
            .accept(&tracing::Span::current())
            .await
            .unwrap()
    });
    (port, accept)
}

/// Server-only transport that accepts QMux over WebSocket and nothing else.
#[derive(Debug)]
#[allow(non_camel_case_types)]
pub(crate) struct WEB_SOCKET_ONLY;

impl TransportProtocol for WEB_SOCKET_ONLY {
    type ConnectionCreator = WebSocketOnlyCreator;
}

pub(crate) struct WebSocketOnlyCreator {
    listener: WebSocketListener,
    port: u16,
}

#[async_trait::async_trait]
impl TransportConnectionCreator for WebSocketOnlyCreator {
    fn client(
        _: u16,
        _: &CertificateVerification,
        _: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("a WebSocket-only endpoint is a server")
    }

    fn server(
        cert_path: &str,
        key_path: &str,
        port: u16,
        _: u64,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        let refusing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let listener = WebSocketListener::bind(port, cert_path, key_path, protocols, refusing)?;
        Ok(Self { listener, port })
    }

    async fn create_new_transport(&self, _: &ConnectTarget) -> anyhow::Result<BoxedConnection> {
        anyhow::bail!("a WebSocket-only endpoint is a server")
    }

    async fn accept_new_transport(&self) -> Result<TransportAccepting, AcceptError> {
        let connection: BoxedConnection = Box::new(self.listener.accept().await?);
        Ok(Box::pin(std::future::ready(Ok(connection))))
    }

    fn refuse_new_connections(&self) {}

    async fn wait_idle(&self) {}

    fn local_port(&self) -> anyhow::Result<u16> {
        Ok(self.port)
    }
}

/// A server reachable only over WebSocket, as behind a network that drops
/// UDP: `_udp` holds the UDP port of the same number and never answers.
pub(crate) struct UdpBlockedServer {
    pub(crate) port: u16,
    pub(crate) server: Endpoint<WEB_SOCKET_ONLY>,
    _udp: std::net::UdpSocket,
}

pub(crate) fn udp_blocked_server(name: &str) -> UdpBlockedServer {
    let config = self_signed_server_config(name);
    loop {
        let udp = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = udp.local_addr().unwrap().port();
        let config = ServerConfig {
            port,
            ..config.clone()
        };
        // Retry: the port UDP picked may already be taken on TCP.
        if let Ok(server) = Endpoint::<WEB_SOCKET_ONLY>::create_server(&config) {
            return UdpBlockedServer {
                port,
                server,
                _udp: udp,
            };
        }
    }
}

/// Forwards to UDP `server_port`, delivering each client datagram `delay` late
/// so QUIC opens only after `delay`. Its TCP port of the same number accepts
/// connections and never answers, so WebSocket never opens.
pub(crate) struct SlowUdpPath {
    pub(crate) port: u16,
    _tcp: std::net::TcpListener,
    forward: tokio::task::JoinHandle<()>,
}

impl Drop for SlowUdpPath {
    fn drop(&mut self) {
        self.forward.abort();
    }
}

pub(crate) async fn slow_udp_path(server_port: u16, delay: Duration) -> SlowUdpPath {
    loop {
        let client_side = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = client_side.local_addr().unwrap().port();
        // Retry: the port UDP picked may already be taken on TCP.
        let Ok(tcp) = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)) else {
            continue;
        };
        let server_side = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        server_side
            .connect((Ipv4Addr::LOCALHOST, server_port))
            .await
            .unwrap();
        let forward = tokio::spawn(forward_late(
            Arc::new(client_side),
            Arc::new(server_side),
            delay,
        ));
        return SlowUdpPath {
            port,
            _tcp: tcp,
            forward,
        };
    }
}

async fn forward_late(
    client_side: Arc<tokio::net::UdpSocket>,
    server_side: Arc<tokio::net::UdpSocket>,
    delay: Duration,
) {
    let mut buffer = vec![0; 65536];
    let Ok((len, client)) = client_side.recv_from(&mut buffer).await else {
        return;
    };
    let replies = {
        let client_side = client_side.clone();
        let server_side = server_side.clone();
        tokio::spawn(async move {
            let mut buffer = vec![0; 65536];
            while let Ok(len) = server_side.recv(&mut buffer).await {
                let _ = client_side.send_to(&buffer[..len], client).await;
            }
        })
    };
    let mut datagram = buffer[..len].to_vec();
    loop {
        let server_side = server_side.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = server_side.send(&datagram).await;
        });
        let Ok((len, _)) = client_side.recv_from(&mut buffer).await else {
            break;
        };
        datagram = buffer[..len].to_vec();
    }
    replies.abort();
}

pub(crate) fn native_client_with_config(config: ClientConfig) -> Endpoint<NATIVE> {
    Endpoint::<NATIVE>::create_client(&config).unwrap()
}

/// `native_client_with_config`, offering only `version`.
pub(crate) fn client_with_config(config: ClientConfig, version: Version) -> Endpoint<NATIVE> {
    native_client_with_config(ClientConfig {
        versions: vec![version],
        ..config
    })
}

pub(crate) fn insecure_client_config() -> ClientConfig {
    ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        follow_go_away: false,
        ..ClientConfig::default()
    }
}

pub(crate) fn native_client() -> Endpoint<NATIVE> {
    native_client_with_config(insecure_client_config())
}

/// Connects a NATIVE client to the server started by `spawn_native_server` and
/// returns both established sessions as (client, server).
pub(crate) async fn connect_sessions(
    url: &str,
    accept: tokio::task::JoinHandle<Session>,
) -> anyhow::Result<(Session, Session)> {
    connect_sessions_from(&native_client(), url, accept).await
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

/// Polls `request` only until `peer` receives what it sends, so its response
/// can arrive without the requester taking it.
pub(crate) async fn send_without_waiting<F: Future + Unpin>(
    request: &mut F,
    peer: &Session,
) -> SessionEvent {
    tokio::select! {
        _ = request => panic!("the request completed before the peer received it"),
        event = within(peer.receive_event()) => event.unwrap(),
    }
}

/// Returns once `receiver` has dispatched every control message `sender`
/// sent before the call: the control stream is ordered, so a later
/// PUBLISH_NAMESPACE reaching `receiver` means the earlier ones were handled.
pub(crate) async fn wait_for_control_delivery(sender: &Session, receiver: &Session) {
    let _pending = sender
        .publisher()
        .begin_publish_namespace("delivery-marker".to_string())
        .await
        .unwrap();
    let SessionEvent::PublishNamespace(_) = within(receiver.receive_event()).await.unwrap() else {
        panic!("expected the PUBLISH_NAMESPACE sent as a delivery marker");
    };
}

pub(crate) async fn receive_disconnected(session: &Session) -> anyhow::Result<()> {
    tokio::time::timeout(DISCONNECT_TIMEOUT, async {
        while !matches!(session.receive_event().await?, SessionEvent::Disconnected()) {}
        anyhow::Ok(())
    })
    .await?
}

pub(crate) async fn spawn_connected_native_sessions(name: &str) -> (Session, Session) {
    let (port, accept) = spawn_native_server(name);
    connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
        .await
        .unwrap()
}

/// Connects a NATIVE client and server over raw QUIC that offer only
/// `version`, and returns them as (client, server).
pub(crate) async fn connected_sessions(name: &str, version: Version) -> (Session, Session) {
    connected_sessions_over(name, version, "moqt").await
}

/// `connected_sessions` for draft-18, over raw QUIC for `moqt` or
/// WebTransport for `https`.
pub(crate) async fn spawn_connected_draft18_sessions(
    name: &str,
    scheme: &str,
) -> (Session, Session) {
    connected_sessions_over(name, Version::Draft18, scheme).await
}

async fn connected_sessions_over(name: &str, version: Version, scheme: &str) -> (Session, Session) {
    let (port, accept) = spawn_server(name, version);
    let client = client_with_config(insecure_client_config(), version);
    let path = if scheme == "https" { "/moq" } else { "" };
    connect_sessions_from(
        &client,
        &format!("{scheme}://127.0.0.1:{port}{path}"),
        accept,
    )
    .await
    .unwrap()
}

pub(crate) async fn accept_namespace(server: &Session) {
    let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
        panic!("expected PUBLISH_NAMESPACE");
    };
    handler.ok().await.unwrap();
}

/// Opens on `session` every bidirectional stream the peer allows, so that
/// the peer's stream limit is reached; the streams stay open while the
/// returned value lives.
pub(crate) async fn use_up_bidi_streams(
    session: &Session,
) -> Vec<(BoxedSendStream, BoxedReceiveStream)> {
    let connection = session.publisher().session.connection();
    let mut streams = Vec::new();
    for _ in 0..MAX_CONCURRENT_BIDI_STREAMS {
        streams.push(connection.transport.open_bi().await.unwrap());
    }
    streams
}

pub(crate) async fn spawn_connected_native_sessions_following_go_away(
    name: &str,
) -> (Session, Session) {
    let (port, accept) = spawn_native_server(name);
    let endpoint = native_client_with_config(ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        follow_go_away: true,
        ..ClientConfig::default()
    });
    connect_sessions_from(&endpoint, &format!("moqt://127.0.0.1:{port}"), accept)
        .await
        .unwrap()
}

pub(crate) async fn subscribe_namespace_accepted(client: &Session, server: &Session, prefix: &str) {
    let subscriber = client.subscriber();
    let (subscribed, ()) = tokio::join!(
        subscriber.subscribe_namespace(prefix.into(), NamespaceInterest::NamespacesAndTracks),
        async {
            let SessionEvent::SubscribeNameSpace(handler) =
                within(server.receive_event()).await.unwrap()
            else {
                panic!("expected SUBSCRIBE_NAMESPACE");
            };
            handler.ok().await.unwrap();
        }
    );
    subscribed.unwrap();
}

/// Answers the client's PUBLISH with PUBLISH_OK and registers the object
/// receiver for its track alias.
pub(crate) async fn accept_publish(server: &Session) -> Subscription {
    let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
        panic!("expected PUBLISH from the client");
    };
    let subscription = handler
        .ok(128, FilterType::LargestObject, true)
        .await
        .unwrap();
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
    let (client, server) = spawn_connected_native_sessions(name).await;
    published_track_between(client, server).await
}

/// `published_track` on a session that speaks `version`.
pub(crate) async fn published_track_over(name: &str, version: Version) -> PublishedTrack {
    let (client, server) = connected_sessions(name, version).await;
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

/// The client subscribes to `ns/track` over `version` and the server accepts it.
pub(crate) async fn subscribed_track(name: &str, version: Version) -> SubscribedTrack {
    let (client, server) = connected_sessions(name, version).await;
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
        // In ascending key order, as draft-18 delta-encodes them (§1.4.3).
        extension_headers: ExtensionHeaders::new(vec![
            KeyValuePair {
                key: 0x0d,
                value: VariantType::Odd(Bytes::from_static(b"\x00\xff\x7f")),
            },
            KeyValuePair {
                key: 0x10,
                value: VariantType::Even(16_384),
            },
            KeyValuePair {
                key: 0x21,
                value: VariantType::Odd(Bytes::from_static(b"\x01\x00\x80")),
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

/// Subgroup 0 of a group; `send_payload` numbers its objects from 0.
pub(crate) struct OpenSubgroup {
    pub(crate) sender: SubgroupObjectSender,
    group_id: u64,
    next_object_id: u64,
}

/// Opens subgroup 0 of `group_id` in the draft of the session `factory`
/// sends on.
pub(crate) async fn open_subgroup(
    factory: &StreamDataSenderFactory,
    group_id: u64,
) -> OpenSubgroup {
    let sender = factory
        .next()
        .await
        .unwrap()
        .start(SubgroupStart {
            group_id,
            subgroup_id: 0,
            publisher_priority: PUBLISHER_PRIORITY,
            ends_group: false,
        })
        .await
        .unwrap();
    OpenSubgroup {
        sender,
        group_id,
        next_object_id: 0,
    }
}

pub(crate) async fn send_payload(subgroup: &mut OpenSubgroup, payload: &'static [u8]) {
    let object = Object {
        location: Location {
            group_id: subgroup.group_id,
            object_id: subgroup.next_object_id,
        },
        forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
        publisher_priority: PUBLISHER_PRIORITY,
        status: ObjectStatus::Normal,
        extension_headers: ExtensionHeaders::default(),
        payload: Bytes::from_static(payload),
        ends_group: false,
    };
    subgroup.next_object_id += 1;
    subgroup.sender.send_object(&object).await.unwrap();
}

/// Opens a subgroup with a draft-14 header written field by field, for
/// tests of the draft-14 encodings; panics on a draft-18 session.
pub(crate) async fn open_raw_subgroup(
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

/// Sends a draft-14 object with Object ID Delta 0; panics on a draft-18
/// session.
pub(crate) async fn send_raw_payload(sender: &mut SubgroupObjectSender, payload: &'static [u8]) {
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
) -> StreamDataReceiver {
    next_subgroup(&mut data_receiver(server, subscription).await).await
}

pub(crate) async fn data_receiver(server: &Session, subscription: &Subscription) -> DataReceiver {
    server
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .unwrap()
}

pub(crate) async fn next_subgroup(receiver: &mut DataReceiver) -> StreamDataReceiver {
    match receiver.next().await.unwrap() {
        TrackData::Subgroup(stream) => stream,
        TrackData::Datagram(object) => panic!("expected a subgroup stream, got {object:?}"),
    }
}

pub(crate) async fn next_datagram(receiver: &mut DataReceiver) -> Object {
    match receiver.next().await.unwrap() {
        TrackData::Datagram(object) => object,
        TrackData::Subgroup(stream) => panic!("expected a datagram, got {stream:?}"),
    }
}

pub(crate) async fn subscribed_track_reader(
    server: &Session,
    subscription: &Subscription,
) -> TrackReader {
    TrackReader::new(data_receiver(server, subscription).await)
}

/// The server side of `track` FETCHes the track and the client answers
/// with `answer`; returns what the FETCH receiver reads first.
pub(crate) async fn fetch_answered_with(
    track: &PublishedTrack,
    answer: &Object,
) -> anyhow::Result<Fetch> {
    let mut subscriber = track.server.subscriber();
    let end = location(answer.location.group_id + 1, 0);
    let (handle, sender) = within(async {
        tokio::join!(
            subscriber.fetch(
                "ns".into(),
                "track".into(),
                location(0, 0),
                end,
                FetchOption::default(),
            ),
            async {
                // An earlier FETCH whose receiver is gone may be cancelled first.
                let handler = loop {
                    match track.client.receive_event().await.unwrap() {
                        SessionEvent::Fetch(handler) => break handler,
                        SessionEvent::FetchCancel(_) => continue,
                        _ => panic!("expected FETCH from the server"),
                    }
                };
                handler.ok(false, end).await.unwrap();
                track
                    .client
                    .publisher()
                    .create_fetch_stream(handler.request_id)
                    .await
                    .unwrap()
            }
        )
    })
    .await;
    sender.send_object(answer).await.unwrap();
    let mut receiver = within(subscriber.accept_fetch_receiver(&handle.unwrap()))
        .await
        .unwrap();
    within(receiver.receive()).await
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
) -> IncomingObjectReceiver {
    context
        .register_data_receiver(&context.connection(), request_id, track_alias)
        .await
        .unwrap();
    take_data_receiver(context, track_alias).await
}

pub(crate) async fn take_data_receiver(
    context: &SessionContext,
    track_alias: u64,
) -> IncomingObjectReceiver {
    context
        .receiver_map
        .lock()
        .await
        .remove(&track_alias)
        .unwrap()
        .receiver
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
        track_properties: Default::default(),
        last_update: crate::LastSubscribeUpdate::default(),
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
    let (client, server) = spawn_connected_native_sessions(name).await;
    fetched_stream_between(client, server).await
}

/// `fetched_stream` on a draft-18 session.
pub(crate) async fn fetched_draft18_stream(name: &str) -> FetchedStream {
    let (client, server) = spawn_connected_draft18_sessions(name, "moqt").await;
    fetched_stream_between(client, server).await
}

async fn fetched_stream_between(client: Session, server: Session) -> FetchedStream {
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

/// A captured span or event: its name (an event's message), its fields
/// formatted with `Debug`, and, for an event, the name of the span it was
/// written in.
#[derive(Clone)]
pub(crate) struct Captured {
    name: String,
    fields: HashMap<String, String>,
    span: Option<String>,
}

impl Captured {
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    pub(crate) fn span(&self) -> Option<&str> {
        self.span.as_deref()
    }
}

/// Install it with `capture_telemetry`.
#[derive(Clone, Default)]
pub(crate) struct CapturedTelemetry {
    closed_spans: Arc<Mutex<Vec<Captured>>>,
    events: Arc<Mutex<Vec<Captured>>>,
}

impl CapturedTelemetry {
    pub(crate) fn closed_spans(&self, name: &str) -> Vec<Captured> {
        named(&self.closed_spans, name)
    }

    pub(crate) fn events(&self, message: &str) -> Vec<Captured> {
        named(&self.events, message)
    }
}

fn named(captured: &Mutex<Vec<Captured>>, name: &str) -> Vec<Captured> {
    let captured = captured.lock().unwrap();
    captured
        .iter()
        .filter(|captured| captured.name == name)
        .cloned()
        .collect()
}

/// Installs a `CapturedTelemetry` as this thread's subscriber until the guard
/// drops; on a current-thread runtime it captures the spawned tasks too.
pub(crate) fn capture_telemetry() -> (CapturedTelemetry, DefaultGuard) {
    let telemetry = CapturedTelemetry::default();
    let subscriber = tracing_subscriber::registry().with(telemetry.clone());
    (telemetry, tracing::subscriber::set_default(subscriber))
}

#[derive(Default)]
struct FieldMap(HashMap<String, String>);

impl Visit for FieldMap {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S> Layer<S> for CapturedTelemetry
where
    S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = FieldMap::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(fields) = span.extensions_mut().get_mut::<FieldMap>()
        {
            values.record(fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = FieldMap::default();
        event.record(&mut fields);
        let name = fields.0.remove("message").unwrap_or_default();
        self.events.lock().unwrap().push(Captured {
            name,
            fields: fields.0,
            span: ctx.event_span(event).map(|span| span.name().to_string()),
        });
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let fields = span
            .extensions_mut()
            .remove::<FieldMap>()
            .unwrap_or_default();
        self.closed_spans.lock().unwrap().push(Captured {
            name: span.name().to_string(),
            fields: fields.0,
            span: None,
        });
    }
}

thread_local! {
    static SERVER_DECLARES_TRACE_CONTEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether a server on this thread puts TRACE_CONTEXT in its SERVER_SETUP.
pub(crate) fn server_declares_trace_context() -> bool {
    SERVER_DECLARES_TRACE_CONTEXT.get()
}

/// Makes the servers of this thread's current-thread runtime answer with a
/// SERVER_SETUP that does not declare TRACE_CONTEXT.
pub(crate) fn without_trace_context_declaration() {
    SERVER_DECLARES_TRACE_CONTEXT.set(false);
}
