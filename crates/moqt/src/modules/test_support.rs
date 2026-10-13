use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bytes::Bytes;
use rcgen::{CertifiedKey, generate_simple_self_signed};
use tracing_subscriber::EnvFilter;

use crate::{
    ClientConfig, DUAL, Endpoint, Session, SessionEvent, Subscription, TrackReader,
    TransportProtocol,
    fetch::{Fetch, FetchDataReceiver, FetchObjectField},
    modules::moqt::{
        data_plane::object::{
            datagram_field::{DatagramField, ObjectDatagramPayload},
            object_datagram::ObjectDatagram,
        },
        domains::session_context::{IncomingObjectNotification, SessionContext},
        runtime::dispatch::incoming_object::IncomingObject,
    },
    object::{
        DataReceiver, ExtensionHeaders, StreamDataSenderFactory, SubgroupId, SubgroupObject,
        SubgroupObjectSender,
    },
    publish::PublishOption,
    server::Handshake,
    server::ServerConfig,
    subscribe::FilterType,
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

fn write_self_signed_cert(dir: &Path) -> ServerConfig {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
    ServerConfig {
        port: 0,
        cert_path: cert_path.to_string_lossy().into_owned(),
        key_path: key_path.to_string_lossy().into_owned(),
        keep_alive_interval_sec: 5,
    }
}

/// Starts a DUAL server on a free port whose accept loop resolves the first
/// incoming handshake (CLIENT_SETUP received, SERVER_SETUP not yet sent);
/// returns the port and the accept task.
pub(crate) fn spawn_dual_server_handshake(name: &str) -> (u16, tokio::task::JoinHandle<Handshake>) {
    init_test_tracing();
    let server_config = write_self_signed_cert(&unique_cert_dir(name));
    let mut server = Endpoint::<DUAL>::create_server(&server_config).unwrap();
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

pub(crate) fn dual_client_with_config(config: ClientConfig) -> Endpoint<DUAL> {
    Endpoint::<DUAL>::create_client(&config).unwrap()
}

pub(crate) fn insecure_client_config() -> ClientConfig {
    ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: None,
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

/// Answers the client's PUBLISH with PUBLISH_OK and registers the object
/// receiver for its track alias.
pub(crate) async fn accept_publish(server: &Session) -> Subscription {
    let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
        panic!("expected PUBLISH from the client");
    };
    let subscription = handler.ok(128, FilterType::LargestObject, 0).await.unwrap();
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

pub(crate) async fn open_subgroup(
    factory: &StreamDataSenderFactory,
    group_id: u64,
    subgroup_id: SubgroupId,
) -> SubgroupObjectSender {
    let uninitialized = factory.next().await.unwrap();
    let header =
        uninitialized.create_header(group_id, subgroup_id, PUBLISHER_PRIORITY, false, false);
    uninitialized.send_header(header).await.unwrap()
}

pub(crate) async fn send_payload(sender: &mut SubgroupObjectSender, payload: &'static [u8]) {
    let field = sender.create_object_field(
        0,
        ExtensionHeaders::default(),
        SubgroupObject::new_payload(Bytes::from_static(payload)),
    );
    sender.send(field).await.unwrap();
}

/// The data receiver resolves only once the first object has arrived, so
/// this must run after the publisher has sent something.
pub(crate) async fn subscribed_track_reader(
    server: &Session,
    subscription: &Subscription,
) -> TrackReader {
    let DataReceiver::Stream(factory) = server
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .unwrap()
    else {
        panic!("expected a subgroup stream receiver");
    };
    TrackReader::new(factory)
}

pub(crate) async fn read_fetch_objects(receiver: &mut FetchDataReceiver) -> Vec<FetchObjectField> {
    let mut objects = Vec::new();
    loop {
        match receiver.receive().await.unwrap() {
            Fetch::Header(_) => {}
            Fetch::Object(object) => objects.push(object),
            Fetch::End => return objects,
        }
    }
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
        .notify_incoming_object(
            track_alias,
            IncomingObject::Datagram(object_datagram(track_alias)),
        )
        .await
}

pub(crate) async fn register_and_take_data_receiver(
    context: &SessionContext,
    request_id: u64,
    track_alias: u64,
) -> tokio::sync::mpsc::UnboundedReceiver<IncomingObject> {
    context
        .register_data_receiver(request_id, track_alias)
        .await
        .unwrap();
    context
        .receiver_map
        .lock()
        .await
        .remove(&track_alias)
        .unwrap()
}
