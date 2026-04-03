use anyhow::{Context, Result, bail};
use bytes::Bytes;
use relay::run_relay_server;
use std::{
    net::ToSocketAddrs,
    path::PathBuf,
    sync::{
        Once,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};

pub const TEST_TRACK_NAMESPACE: &str = "room/member";
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

static PORT_NUMBER: AtomicU16 = AtomicU16::new(1000);
static LOG_INITIALIZER: Once = Once::new();

#[derive(Clone, Copy)]
pub enum PublisherServeMode {
    StreamFiveObjects,
    DatagramFiveObjects,
    StreamReopenFiveTimes,
}

pub fn get_port() -> u16 {
    PORT_NUMBER.fetch_add(1, Ordering::SeqCst)
}

fn log_init() {
    LOG_INITIALIZER.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_line_number(true)
            .try_init()
            .ok();
    });
}

fn resolve_key_path(file_name: &str) -> PathBuf {
    let current = std::env::current_dir().unwrap();
    let candidates = [
        current.join("keys").join(file_name),
        current.join("..").join("keys").join(file_name),
    ];
    for candidate in candidates {
        if candidate.exists() {
            return candidate;
        }
    }
    current.join("keys").join(file_name)
}

pub fn get_cert_path() -> PathBuf {
    resolve_key_path("cert.pem")
}

pub fn get_key_path() -> PathBuf {
    resolve_key_path("key.pem")
}

pub fn activate_server<T: moqt::TransportProtocol>(
    port_num: u16,
    receiver: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let key_path = get_key_path();
    let cert_path = get_cert_path();
    log_init();
    run_relay_server::<T>(
        port_num,
        receiver,
        key_path.to_str().unwrap(),
        cert_path.to_str().unwrap(),
    )
}

pub async fn connect_session_quic(port_num: u16) -> Result<moqt::Session<moqt::QUIC>> {
    let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client_with_custom_cert(
        0,
        get_cert_path().to_str().unwrap(),
    )?;
    let host = "localhost";
    let remote_address = (host, port_num)
        .to_socket_addrs()
        .context("failed to resolve remote address")?
        .next()
        .context("no remote address resolved")?;
    let session = endpoint.connect(remote_address, host).await?;
    Ok(session)
}

fn extension_headers() -> moqt::ExtensionHeaders {
    moqt::ExtensionHeaders {
        prior_group_id_gap: vec![],
        prior_object_id_gap: vec![],
        immutable_extensions: vec![],
    }
}

async fn send_stream_objects(
    publisher: &moqt::Publisher<moqt::QUIC>,
    publication: &moqt::PublishedResource,
    group_id: u64,
    object_count: u64,
) -> Result<()> {
    let mut stream = publisher.create_stream(publication).next().await?;
    let header = stream.create_header(group_id, moqt::SubgroupId::None, 128, false, false);
    for object_id in 0..object_count {
        let payload =
            moqt::SubgroupObject::new_payload(Bytes::from(format!("stream-object-{}", object_id)));
        let object = stream.create_object_field(&header, object_id, extension_headers(), payload);
        stream.send(&header, object).await?;
    }
    Ok(())
}

async fn send_datagram_objects(
    publisher: &moqt::Publisher<moqt::QUIC>,
    publication: &moqt::PublishedResource,
    object_count: u64,
) -> Result<()> {
    let mut datagram = publisher.create_datagram(publication);
    for object_id in 0..object_count {
        let field = moqt::DatagramField::Payload0x00 {
            object_id,
            publisher_priority: 128,
            payload: moqt::DatagramField::to_bytes(format!("datagram-object-{}", object_id)),
        };
        let object = datagram.create_object_datagram(0, field);
        datagram.send(object).await?;
    }
    Ok(())
}

pub async fn serve_publisher_side(
    session: moqt::Session<moqt::QUIC>,
    track_name: String,
    mode: PublisherServeMode,
) -> Result<()> {
    let publisher = session.publisher();
    loop {
        let event = tokio::time::timeout(TEST_TIMEOUT, session.receive_event())
            .await
            .context("timed out waiting publisher-side session event")??;
        match event {
            moqt::SessionEvent::Subscribe(handler) => {
                if handler.track_namespace != TEST_TRACK_NAMESPACE
                    || handler.track_name != track_name
                {
                    continue;
                }

                let track_alias = handler.ok(60, moqt::ContentExists::False).await?;
                let publication = handler.into_publication(track_alias);
                match mode {
                    PublisherServeMode::StreamFiveObjects => {
                        send_stream_objects(&publisher, &publication, 0, 5).await?;
                    }
                    PublisherServeMode::DatagramFiveObjects => {
                        send_datagram_objects(&publisher, &publication, 5).await?;
                    }
                    PublisherServeMode::StreamReopenFiveTimes => {
                        for group_id in 0..5 {
                            send_stream_objects(&publisher, &publication, group_id, 1).await?;
                        }
                    }
                }
                return Ok(());
            }
            moqt::SessionEvent::PublishNamespace(handler) => {
                let _ = handler.ok().await;
            }
            moqt::SessionEvent::SubscribeNameSpace(handler) => {
                let _ = handler.ok().await;
            }
            moqt::SessionEvent::Publish(handler) => {
                let _ = handler.ok(128, moqt::FilterType::LargestObject).await;
            }
            moqt::SessionEvent::ProtocolViolation() => {
                bail!("publisher-side session protocol violation")
            }
        }
    }
}

fn subscribe_option() -> moqt::SubscribeOption {
    moqt::SubscribeOption {
        subscriber_priority: 128,
        group_order: moqt::GroupOrder::Ascending,
        forward: true,
        filter_type: moqt::FilterType::LargestObject,
    }
}

pub async fn assert_receive_stream_objects(
    mut subscriber: moqt::Subscriber<moqt::QUIC>,
    track_name: &str,
    expected_objects: usize,
) -> Result<()> {
    let subscription = subscriber
        .subscribe(
            TEST_TRACK_NAMESPACE.to_string(),
            track_name.to_string(),
            subscribe_option(),
        )
        .await?;
    let receiver =
        tokio::time::timeout(TEST_TIMEOUT, subscriber.accept_data_receiver(&subscription))
            .await
            .context("timed out waiting stream data receiver")??;

    match receiver {
        moqt::DataReceiver::Stream(mut factory) => {
            let mut stream = factory
                .next()
                .await
                .context("failed to get initial stream")?;
            let mut object_count = 0usize;
            while object_count < expected_objects {
                let subgroup = tokio::time::timeout(TEST_TIMEOUT, stream.receive())
                    .await
                    .context("timed out waiting stream subgroup")??;
                if let moqt::Subgroup::Object(_) = subgroup {
                    object_count += 1;
                }
            }
            assert_eq!(object_count, expected_objects);
            Ok(())
        }
        moqt::DataReceiver::Datagram(_) => {
            bail!("expected stream receiver but got datagram receiver")
        }
    }
}

pub async fn assert_receive_datagram_objects(
    mut subscriber: moqt::Subscriber<moqt::QUIC>,
    track_name: &str,
    expected_objects: usize,
) -> Result<()> {
    let subscription = subscriber
        .subscribe(
            TEST_TRACK_NAMESPACE.to_string(),
            track_name.to_string(),
            subscribe_option(),
        )
        .await?;
    let receiver =
        tokio::time::timeout(TEST_TIMEOUT, subscriber.accept_data_receiver(&subscription))
            .await
            .context("timed out waiting datagram data receiver")??;

    match receiver {
        moqt::DataReceiver::Datagram(mut datagram) => {
            for _ in 0..expected_objects {
                let _object = tokio::time::timeout(TEST_TIMEOUT, datagram.receive())
                    .await
                    .context("timed out waiting datagram object")??;
            }
            Ok(())
        }
        moqt::DataReceiver::Stream(_) => {
            bail!("expected datagram receiver but got stream receiver")
        }
    }
}

pub async fn assert_receive_reopened_streams(
    mut subscriber: moqt::Subscriber<moqt::QUIC>,
    track_name: &str,
    expected_stream_count: usize,
) -> Result<()> {
    let subscription = subscriber
        .subscribe(
            TEST_TRACK_NAMESPACE.to_string(),
            track_name.to_string(),
            subscribe_option(),
        )
        .await?;

    let receiver =
        tokio::time::timeout(TEST_TIMEOUT, subscriber.accept_data_receiver(&subscription))
            .await
            .context("timed out waiting reopened stream receiver")??;

    match receiver {
        moqt::DataReceiver::Stream(mut factory) => {
            for _ in 0..expected_stream_count {
                let mut stream = factory.next().await.context("failed to get stream")?;
                let first = tokio::time::timeout(TEST_TIMEOUT, stream.receive())
                    .await
                    .context("timed out waiting stream header")??;
                match first {
                    moqt::Subgroup::Header(header) => {
                        assert_eq!(header.track_alias, subscription.track_alias);
                    }
                    moqt::Subgroup::Object(_) => {
                        bail!("expected stream header as first subgroup")
                    }
                }

                let second = tokio::time::timeout(TEST_TIMEOUT, stream.receive())
                    .await
                    .context("timed out waiting stream object")??;
                match second {
                    moqt::Subgroup::Object(field) => {
                        assert_eq!(field.object_id_delta, 0);
                    }
                    moqt::Subgroup::Header(_) => {
                        bail!("expected stream object as second subgroup")
                    }
                }
            }
            Ok(())
        }
        moqt::DataReceiver::Datagram(_) => {
            bail!("expected stream receiver but got datagram receiver")
        }
    }
}
