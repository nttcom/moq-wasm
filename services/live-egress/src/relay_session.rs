use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use moqt::{
    ClientConfig, Endpoint, NATIVE, Session, Subscription, TrackReader,
    object::DataReceiver,
    subscribe::{FilterType, SubscribeOption},
};

pub async fn connect(url: &str, config: &ClientConfig) -> Result<Arc<Session>> {
    let endpoint = Endpoint::<NATIVE>::create_client(config)?;
    tracing::info!(relay = %url, "connecting to relay");
    let connecting = endpoint
        .connect(url)
        .await
        .context("connect moqt transport")?;
    Ok(Arc::new(
        connecting.await.context("establish moqt session")?,
    ))
}

pub async fn subscribe(
    session: &Session,
    namespace: &str,
    name: &str,
    filter_type: FilterType,
) -> Result<Subscription> {
    let option = SubscribeOption {
        filter_type,
        ..SubscribeOption::default()
    };
    session
        .subscriber()
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("subscribe {namespace}/{name}"))
}

/// Waits for the first data stream of `subscription`.
pub async fn track_reader(session: &Session, subscription: &Subscription) -> Result<TrackReader> {
    let receiver = session
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .context("accept data receiver")?;
    let DataReceiver::Stream(factory) = receiver else {
        bail!("expected a stream data receiver");
    };
    Ok(TrackReader::new(factory))
}

/// Reads the first object of the catalog's current group with the SUBSCRIBE and
/// joining FETCH (offset 0) that draft-ietf-moq-msf-01 §5 requires, or the next
/// object when the track has none yet.
pub async fn read_catalog(session: &Session, namespace: &str) -> Result<Bytes> {
    let (_subscription, mut reader) = session
        .subscriber()
        .subscribe_current_group(namespace.to_string(), msf::CATALOG_TRACK_NAME.to_string())
        .await
        .with_context(|| format!("subscribe {namespace}/{}", msf::CATALOG_TRACK_NAME))?;
    match reader.next_object().await? {
        Some(object) => Ok(object.payload),
        None => bail!("catalog track ended before any object"),
    }
}
