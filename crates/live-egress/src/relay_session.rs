use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use moqt::{
    ClientConfig, DUAL, Endpoint, Session, Subscription, TrackReader,
    fetch::{Fetch, FetchObject, FetchObjectField, FetchOption},
    object::{DataReceiver, GroupOrder},
    subscribe::{FilterType, SubscribeOption},
};

const SUBSCRIBER_PRIORITY: u8 = 128;

pub struct RelaySession {
    pub session: Arc<Session>,
    endpoint: Endpoint<DUAL>,
}

impl RelaySession {
    pub async fn connect(url: &str, config: &ClientConfig) -> Result<Self> {
        let endpoint = Endpoint::<DUAL>::create_client(config)?;
        tracing::info!(relay = %url, "connecting to relay");
        let connecting = endpoint
            .connect(url)
            .await
            .context("connect moqt transport")?;
        let session = Arc::new(connecting.await.context("establish moqt session")?);
        Ok(Self { session, endpoint })
    }

    /// Every other handle to the session must already be dropped, or the
    /// wait for the close to be sent never ends.
    pub async fn close(self) {
        drop(self.session);
        self.endpoint.wait_idle().await;
    }
}

/// Subscribes from the next group start, so a media track begins at a keyframe.
pub async fn subscribe(session: &Session, namespace: &str, name: &str) -> Result<Subscription> {
    let option = SubscribeOption {
        subscriber_priority: SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type: FilterType::NextGroupStart,
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

/// Reads the catalog's latest object with the joining FETCH (offset 0) that
/// draft-ietf-moq-msf-01 §5 requires beside the SUBSCRIBE, falling back to the
/// subscription's next group when the FETCH fails, e.g. before the first
/// catalog was sent.
pub async fn read_catalog(session: &Session, subscription: &Subscription) -> Result<Bytes> {
    match fetch_joining_group(session, subscription.request_id()).await {
        Ok(Some(latest)) => return Ok(latest),
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "joining FETCH of the catalog failed"),
    }
    let mut updates = track_reader(session, subscription).await?;
    match updates.next_object().await? {
        Some(object) => Ok(object.payload),
        None => bail!("catalog track ended before any object"),
    }
}

/// The last object of the group the subscription `request_id` joined at.
async fn fetch_joining_group(session: &Session, request_id: u64) -> Result<Option<Bytes>> {
    let mut subscriber = session.subscriber();
    let handle = subscriber
        .fetch_relative_joining(request_id, 0, FetchOption::default())
        .await?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    let mut latest = None;
    loop {
        match receiver.receive().await? {
            Fetch::Header(_) => {}
            Fetch::Object(FetchObjectField {
                fetch_object: FetchObject::Payload(payload),
                ..
            }) => latest = Some(payload),
            Fetch::Object(_) => {}
            Fetch::End => return Ok(latest),
        }
    }
}
