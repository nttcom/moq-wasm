use anyhow::{Context, Result};
use moqt::{
    ClientConfig, DataReceiver, Endpoint, FilterType, GroupOrder, QUIC, Session,
    StreamDataReceiverFactory, SubscribeOption,
};
use tracing::info;

use url::Url;

const SUBSCRIBER_PRIORITY: u8 = 128;

pub async fn connect_session(relay: &Url, insecure: bool) -> Result<Session<QUIC>> {
    let config = ClientConfig {
        port: 0,
        verify_certificate: !insecure,
        authorization_token: None,
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    info!(%relay, "connecting to relay");
    let connecting = endpoint.connect(relay.as_str()).await?;
    let session = connecting.await?;
    Ok(session)
}

pub async fn subscribe_track(
    session: &Session<QUIC>,
    namespace: &str,
    name: &str,
) -> Result<StreamDataReceiverFactory<QUIC>> {
    let option = SubscribeOption {
        subscriber_priority: SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::NextGroupStart,
    };
    let subscription = session
        .subscriber()
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))?;
    let receiver = session
        .subscriber()
        .accept_data_receiver(&subscription)
        .await
        .context("failed to accept data receiver")?;
    let DataReceiver::Stream(factory) = receiver else {
        anyhow::bail!("expected stream data receiver");
    };
    Ok(factory)
}
