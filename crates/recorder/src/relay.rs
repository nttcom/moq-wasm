use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{
    ClientConfig, DUAL, Endpoint, Session, Subscription, TrackReader,
    fetch::{Fetch, FetchObject, FetchObjectField, FetchOption},
    object::{DataReceiver, GroupOrder},
    subscribe::{FilterType, SubscribeOption},
};

const SUBSCRIBER_PRIORITY: u8 = 128;

pub(crate) struct RelayConnection {
    pub(crate) session: Arc<Session>,
    endpoint: Endpoint<DUAL>,
}

impl RelayConnection {
    pub(crate) async fn connect(
        url: &str,
        verify_certificate: bool,
        authorization_token: Option<String>,
    ) -> Result<Self> {
        let config = ClientConfig {
            port: 0,
            verify_certificate,
            authorization_token,
        };
        let endpoint = Endpoint::<DUAL>::create_client(&config)?;
        tracing::info!(relay = url, "connecting to relay");
        let connecting = endpoint
            .connect(url)
            .await
            .with_context(|| format!("cannot connect to {url}"))?;
        let session = Arc::new(connecting.await?);
        Ok(Self { session, endpoint })
    }

    /// Every other handle to the session must already be dropped, or the wait
    /// never ends. Waiting lets the CONNECTION_CLOSE reach the relay before the
    /// process exits.
    pub(crate) async fn close(self) {
        drop(self.session);
        self.endpoint.wait_idle().await;
    }
}

/// Subscribes from the next group start, so a recording begins on a keyframe.
pub(crate) async fn subscribe(
    session: &Session,
    namespace: &str,
    name: &str,
) -> Result<Subscription> {
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
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))
}

/// Waits for the first data stream of `subscription`.
pub(crate) async fn track_reader(
    session: &Session,
    subscription: &Subscription,
) -> Result<TrackReader> {
    let receiver = session
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .context("failed to accept data receiver")?;
    let DataReceiver::Stream(factory) = receiver else {
        anyhow::bail!("expected stream data receiver");
    };
    Ok(TrackReader::new(factory))
}

/// The last object of the group `subscription` joined at: the joining FETCH
/// with offset 0 that draft-ietf-moq-msf-01 §5 asks for beside a catalog
/// SUBSCRIBE. `None` when the publisher has sent nothing yet.
pub(crate) async fn fetch_joining_group(
    session: &Session,
    subscription: &Subscription,
) -> Result<Option<Bytes>> {
    let mut subscriber = session.subscriber();
    let handle = subscriber
        .fetch_relative_joining(subscription.request_id(), 0, FetchOption::default())
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
