use std::net::ToSocketAddrs;
use std::str::FromStr;

use moqt::{
    ContentExists, DataReceiver, Endpoint, ExtensionHeaders, Fetch, FetchDataReceiver, FetchObject,
    FetchOption, FilterType, GroupOrder, Location, Session, StreamDataReceiverFactory,
    StreamDataSenderFactory, Subgroup, SubgroupId, SubgroupObject, SubscribeOption, QUIC,
};

const RELAY_URL: &str = "moqt://localhost:4433";
const NAMESPACE: &str = "room/alice";
const TRACK_NAME: &str = "data";
const PUBLISHER_PRIORITY: u8 = 128;
const GROUPS: u64 = 3;
const OBJECTS_PER_GROUP: u64 = 5;

async fn new_session(cert_path: &str) -> anyhow::Result<Session<QUIC>> {
    let url = url::Url::from_str(RELAY_URL).unwrap();
    let host = url.host_str().unwrap();
    let remote = (host, url.port().unwrap_or(4433))
        .to_socket_addrs()?
        .next()
        .unwrap();
    let endpoint = Endpoint::<QUIC>::create_client_with_custom_cert(0, cert_path)?;
    let connecting = endpoint.connect(remote, host).await?;
    Ok(connecting.await?)
}

async fn send_group(factory: &StreamDataSenderFactory<QUIC>, group_id: u64) -> anyhow::Result<()> {
    let sender = factory.next().await?;
    let header = sender.create_header(group_id, SubgroupId::None, PUBLISHER_PRIORITY, false, false);
    let mut stream = sender.send_header(header).await?;
    for obj_id in 0..OBJECTS_PER_GROUP {
        let payload = format!("g{}:o{}", group_id, obj_id);
        let obj = stream.create_object_field(
            0,
            ExtensionHeaders {
                prior_group_id_gap: vec![],
                prior_object_id_gap: vec![],
                immutable_extensions: vec![],
            },
            SubgroupObject::new_payload(payload.into()),
        );
        stream.send(obj).await?;
        tracing::info!("[alice] sent g{}:o{}", group_id, obj_id);
    }
    stream.close().await
}

async fn run_fetch(
    subscriber: &mut moqt::Subscriber<QUIC>,
    start: Location,
    end: Location,
) -> anyhow::Result<()> {
    tracing::info!(
        "[bob] fetching g{}:o{}..g{}:o{}",
        start.group_id, start.object_id, end.group_id, end.object_id
    );
    let handle = subscriber
        .fetch(
            NAMESPACE.to_string(),
            TRACK_NAME.to_string(),
            start,
            end,
            FetchOption::default(),
        )
        .await?;
    let mut receiver: FetchDataReceiver<QUIC> = subscriber.accept_fetch_receiver(&handle).await?;
    loop {
        match receiver.receive().await {
            Ok(Fetch::Header(_)) => {}
            Ok(Fetch::Object(obj)) => {
                let payload = match &obj.fetch_object {
                    FetchObject::Payload(b) => String::from_utf8_lossy(b).to_string(),
                    FetchObject::Status(s) => format!("{:?}", s),
                };
                tracing::info!("[bob] g{}:o{} = {}", obj.group_id, obj.object_id, payload);
            }
            Ok(Fetch::End) => {
                tracing::info!(
                    "[bob] fetch done g{}:o{}..g{}:o{}",
                    start.group_id, start.object_id, end.group_id, end.object_id
                );
                break;
            }
            Err(e) => {
                tracing::info!(
                    "[bob] fetch done (error) g{}:o{}..g{}:o{}: {}",
                    start.group_id, start.object_id, end.group_id, end.object_id, e
                );
                break;
            }
        }
    }
    Ok(())
}

async fn alice(cert_path: String) -> anyhow::Result<()> {
    let session = new_session(&cert_path).await?;
    session
        .publisher()
        .publish_namespace(NAMESPACE.to_string())
        .await?;
    tracing::info!("[alice] publish_namespace ok");

    loop {
        match session.receive_event().await? {
            moqt::SessionEvent::Subscribe(handler) => {
                tracing::info!(
                    "[alice] received Subscribe for {}/{}",
                    handler.track_namespace,
                    handler.track_name
                );
                let track_alias = handler.ok(0, ContentExists::False).await?;
                let publication = handler.into_publication(track_alias);
                let factory = session.publisher().create_stream(&publication);
                for group_id in 0..GROUPS {
                    send_group(&factory, group_id).await?;
                }
                tracing::info!("[alice] all groups published");
            }
            moqt::SessionEvent::Disconnected() => break,
            _ => {}
        }
    }
    Ok(())
}

async fn bob(cert_path: String) -> anyhow::Result<()> {
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let session = new_session(&cert_path).await?;

    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            NAMESPACE.to_string(),
            TRACK_NAME.to_string(),
            SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LargestObject,
            },
        )
        .await?;
    tracing::info!("[bob] subscribe ok, track_alias={}", subscription.track_alias);

    let data_receiver = subscriber.accept_data_receiver(&subscription).await?;
    let mut factory: StreamDataReceiverFactory<QUIC> = match data_receiver {
        DataReceiver::Stream(f) => f,
        DataReceiver::Datagram(_) => anyhow::bail!("[bob] unexpected datagram"),
    };

    'detect: loop {
        let mut stream = factory.next().await?;
        loop {
            match stream.receive().await {
                Ok(Subgroup::Header(h)) => {
                    tracing::info!("[bob] live group_id={}", h.group_id);
                    if h.group_id >= 2 {
                        tracing::info!("[bob] group 2 detected, relay cache ready");
                        break 'detect;
                    }
                }
                Ok(Subgroup::Object(_)) => {}
                Err(_) => break, // stream ended, move to next group
            }
        }
    }

    tracing::info!("[bob] detect done, issuing fetches");

    // fetch A: g0/o0 .. g1/o2 (8 objects)
    run_fetch(&mut subscriber, Location { group_id: 0, object_id: 0 }, Location { group_id: 1, object_id: 2 }).await?;
    // fetch B: g1/o2 .. g2/o3 (7 objects)
    run_fetch(&mut subscriber, Location { group_id: 1, object_id: 2 }, Location { group_id: 2, object_id: 3 }).await?;
    // fetch C: g1/o0 .. g1/o0 = entire group 1 (5 objects)
    run_fetch(&mut subscriber, Location { group_id: 1, object_id: 0 }, Location { group_id: 1, object_id: 0 }).await?;

    tracing::info!("[bob] all done");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_line_number(true)
        .try_init()
        .ok();

    let cert_path = format!(
        "{}/relay/keys/cert.pem",
        std::env::current_dir()?.to_str().unwrap()
    );

    let alice_handle = tokio::spawn(alice(cert_path.clone()));
    let bob_handle = tokio::spawn(bob(cert_path));

    tokio::select! {
        r = alice_handle => { r??; }
        r = bob_handle => { r??; }
        _ = tokio::signal::ctrl_c() => {}
    }

    Ok(())
}
