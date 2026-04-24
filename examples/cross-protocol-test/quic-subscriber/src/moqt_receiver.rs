use std::net::ToSocketAddrs;
use std::str::FromStr;

use anyhow::{Context as _, Result};
use moqt::{
    ClientConfig, Endpoint, FilterType, GroupOrder, QUIC, SessionEvent, Subgroup, SubgroupObject,
    SubscribeOption,
};
use std::io::Write;
use tracing::info;

pub async fn subscribe_and_receive(namespace: &str, track_name: &str) -> Result<()> {
    let config = ClientConfig {
        port: 0,
        verify_certificate: false,
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    let url = url::Url::from_str("moqt://localhost:4433")?;
    let host = url.host_str().unwrap();
    let remote_address = (host, url.port().unwrap())
        .to_socket_addrs()?
        .next()
        .context("failed to resolve address")?;

    info!(%remote_address, "connecting to relay via QUIC");
    let connecting = endpoint.connect(remote_address, host).await?;
    let session = connecting.await?;

    let (_publisher, subscriber) = session.publisher_subscriber_pair();
    let _subscriber = std::sync::Arc::new(subscriber);
    let session = std::sync::Arc::new(session);

    // イベント処理タスク
    tokio::spawn({
        let session = session.clone();
        async move {
            loop {
                let event = match session.receive_event().await {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("event loop error: {}", e);
                        break;
                    }
                };
                if let SessionEvent::ProtocolViolation() = event {
                    tracing::error!("protocol violation");
                    break;
                }
            }
        }
    });

    let option = SubscribeOption {
        subscriber_priority: 128,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::NextGroupStart,
    };

    info!(namespace, track_name, "subscribing");
    let subscription = session
        .subscriber()
        .subscribe(namespace.to_string(), track_name.to_string(), option)
        .await
        .context("failed to subscribe")?;

    info!("waiting for data stream");
    let receiver = session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
        .context("failed to accept data receiver")?;

    match receiver {
        moqt::DataReceiver::Stream(mut factory) => {
            let mut stream = factory
                .next()
                .await
                .context("failed to get initial stream")?;
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let mut init_written = false;
            let mut object_id: u64 = 0;
            loop {
                let result = match stream.receive().await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("receive error: {}", e);
                        break;
                    }
                };
                match result {
                    Subgroup::Header(header) => {
                        info!(group_id = header.group_id, "received subgroup header");
                        object_id = 0;
                    }
                    Subgroup::Object(field) => {
                        let current_object_id = object_id;
                        object_id += 1;
                        match field.subgroup_object {
                            SubgroupObject::Payload { data, .. } => {
                                // Object 0 は init segment。初回のみ書き出す
                                if current_object_id == 0 {
                                    if !init_written {
                                        info!(size = data.len(), "writing init segment");
                                        out.write_all(&data)?;
                                        out.flush()?;
                                        init_written = true;
                                    }
                                    continue;
                                }
                                out.write_all(&data)?;
                                out.flush()?;
                            }
                            SubgroupObject::Status { code, .. } => {
                                info!(code, "received status object");
                            }
                        }
                    }
                }
            }
        }
        moqt::DataReceiver::Datagram(mut datagram) => loop {
            let result = match datagram.receive().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("datagram receive error: {}", e);
                    break;
                }
            };
            info!("received datagram: {:?}", result);
        },
    }

    Ok(())
}
