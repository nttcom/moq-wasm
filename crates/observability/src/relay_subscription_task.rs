use std::time::Duration;

use anyhow::{Context, bail};
use moqt::{
    DataReceiver, FilterType, GroupOrder, Session, SessionEvent, SubscribeOption, TrackReader,
};
use relay_stats::RelaySnapshot;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::config::RelayEndpoint;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);
const SUBSCRIBER_PRIORITY: u8 = 0;

pub struct RelayConnectionOptions {
    pub auth_token: Option<String>,
    pub verify_certificate: bool,
}

pub struct RelaySubscriptionTask {
    join_handle: JoinHandle<()>,
}

impl RelaySubscriptionTask {
    pub fn run(
        relay: RelayEndpoint,
        options: RelayConnectionOptions,
        snapshot_sender: mpsc::Sender<RelaySnapshot>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            loop {
                if let Err(error) = receive_snapshots(&relay, &options, &snapshot_sender).await {
                    tracing::warn!(?error, relay_id = %relay.relay_id, "relay stats subscription ended; reconnecting");
                }
                if snapshot_sender.is_closed() {
                    return;
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        });
        Self { join_handle }
    }
}

impl Drop for RelaySubscriptionTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn receive_snapshots(
    relay: &RelayEndpoint,
    options: &RelayConnectionOptions,
    snapshot_sender: &mpsc::Sender<RelaySnapshot>,
) -> anyhow::Result<()> {
    let endpoint = moqt::Endpoint::<moqt::QUIC>::create_client(&moqt::ClientConfig {
        port: 0,
        verify_certificate: options.verify_certificate,
        authorization_token: options.auth_token.clone(),
    })?;
    let session = endpoint
        .connect(&relay.url)
        .await?
        .await
        .with_context(|| format!("cannot connect to {}", relay.url))?;
    let mut subscriber = session.subscriber();
    let subscription = subscriber
        .subscribe(
            relay_stats::track_namespace(&relay.relay_id),
            relay_stats::TRACK_NAME.to_string(),
            SubscribeOption {
                subscriber_priority: SUBSCRIBER_PRIORITY,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::NextGroupStart,
            },
        )
        .await
        .context("SUBSCRIBE to the stats track was refused")?;
    let DataReceiver::Stream(factory) = subscriber.accept_data_receiver(&subscription).await?
    else {
        bail!("the stats track is not delivered on streams");
    };
    tracing::info!(relay_id = %relay.relay_id, url = %relay.url, "subscribed to relay stats");
    let mut reader = TrackReader::new(factory);
    let mut closed = std::pin::pin!(session_closed(&session));
    loop {
        let next = tokio::select! {
            next = reader.next_object() => next,
            reason = &mut closed => return Err(reason),
        };
        match next {
            Ok(Some(object)) => match RelaySnapshot::from_json(&object.payload) {
                Ok(snapshot) => snapshot_sender.send(snapshot).await?,
                Err(error) => {
                    tracing::warn!(?error, relay_id = %relay.relay_id, "dropping an unreadable snapshot")
                }
            },
            Ok(None) => bail!("the relay ended the stats track"),
            Err(error) => {
                tracing::warn!(?error, relay_id = %relay.relay_id, "a snapshot group failed")
            }
        }
    }
}

async fn session_closed(session: &Session) -> anyhow::Error {
    loop {
        match session.receive_event().await {
            Ok(SessionEvent::Disconnected()) => {
                return anyhow::anyhow!("the relay closed the session");
            }
            Ok(SessionEvent::ProtocolViolation()) => return anyhow::anyhow!("protocol violation"),
            Ok(_) => {}
            Err(error) => return error.context("session event loop failed"),
        }
    }
}
