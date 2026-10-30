use std::{
    net::Ipv4Addr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::Bytes;
use tracing_subscriber::EnvFilter;

use crate::modules::moqt::control_plane::error_codes::RequestErrorCode;
use crate::modules::moqt::version::APPLICATION_PROTOCOLS;
use crate::modules::transport::{
    accept_error::AcceptError, application_protocol::ApplicationProtocols,
    connect_target::ConnectTarget, transport_connection::BoxedConnection,
    transport_connection_creator::TransportConnectionCreator,
    websocket::websocket_listener::WebSocketListener,
};

use crate::{
    ClientConfig, Endpoint, NATIVE, RequestFailure, Session, SessionEvent, Subscription,
    TrackReader, TransportProtocol,
    fetch::{Fetch, FetchDataReceiver, FetchDataSender, FetchOption},
    modules::moqt::{
        data_plane::object::{
            datagram_field::{DatagramField, ObjectDatagramPayload},
            object_datagram::ObjectDatagram,
        },
        domains::{connection::IncomingObjectNotification, session_context::SessionContext},
        runtime::dispatch::incoming_object::{IncomingObject, IncomingObjectReceiver},
    },
    object::{
        DataReceiver, DatagramReceiver, ExtensionHeaders, ForwardingPreference, KeyValuePair,
        Location, Object, ObjectStatus, StreamDataReceiver, StreamDataReceiverFactory,
        StreamDataSenderFactory, Subgroup, SubgroupObjectSender, SubgroupStart, VariantType,
    },
    publish::PublishOption,
    server::Handshake,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
    subscribe::FilterType,
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

/// Starts a NATIVE server on a free port whose accept loop resolves the first
/// incoming handshake (CLIENT_SETUP received, SERVER_SETUP not yet sent);
/// returns the port and the accept task.
pub(crate) fn spawn_native_server_handshake(
    name: &str,
) -> (u16, tokio::task::JoinHandle<Handshake>) {
    let server_config = self_signed_server_config(name);
    let server = Endpoint::<NATIVE>::create_server(&server_config).unwrap();
    let port = server.local_port().unwrap();
    let accept = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
    (port, accept)
}

/// Starts a NATIVE server on a free port whose accept loop resolves the first
/// incoming session; returns the port and the accept task.
pub(crate) fn spawn_native_server(name: &str) -> (u16, tokio::task::JoinHandle<Session>) {
    let (port, handshake) = spawn_native_server_handshake(name);
    let accept = tokio::spawn(async move { handshake.await.unwrap().accept().await.unwrap() });
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
    fn client(_: u16, _: bool, _: ApplicationProtocols) -> anyhow::Result<Self> {
        anyhow::bail!("a WebSocket-only endpoint is a server")
    }

    fn client_with_custom_cert(_: u16, _: &str, _: ApplicationProtocols) -> anyhow::Result<Self> {
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

    async fn accept_new_transport(&self) -> Result<BoxedConnection, AcceptError> {
        Ok(Box::new(self.listener.accept().await?))
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

pub(crate) fn insecure_client_config() -> ClientConfig {
    ClientConfig {
        verify_certificate: false,
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

pub(crate) async fn subscribe_namespace_accepted(client: &Session, server: &Session, prefix: &str) {
    let subscriber = client.subscriber();
    let (subscribed, ()) = tokio::join!(subscriber.subscribe_namespace(prefix.into()), async {
        let SessionEvent::SubscribeNameSpace(handler) =
            within(server.receive_event()).await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE");
        };
        handler.ok().await.unwrap();
    });
    subscribed.unwrap();
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
    let (client, server) = spawn_connected_native_sessions(name).await;
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
        .notify_incoming_object(track_alias, IncomingObject::Datagram(datagram_object()))
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
