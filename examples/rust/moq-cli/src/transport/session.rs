use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{
    ClientConfig, Endpoint, QUIC, Session, Subscription, TrackReader,
    fetch::{Fetch, FetchOption},
    object::{DataReceiver, GroupOrder, Object},
    subscribe::{FilterType, SubscribeOption},
};
use tracing::info;

use super::{auth_token_file::read_auth_token, auth_token_refresh_task::AuthTokenRefreshTask};
use crate::cli::RelayArgs;

const SUBSCRIBER_PRIORITY: u8 = 128;
pub struct RelayConnection {
    pub session: Arc<Session>,
    auth_token_refresh: Option<AuthTokenRefreshTask>,
    endpoint: Endpoint<QUIC>,
}

impl RelayConnection {
    /// Every other handle to the session (publishers, track writers, clones of
    /// `session`) must already be dropped, or the wait never ends.
    pub async fn close(self) {
        drop(self.auth_token_refresh);
        drop(self.session);
        self.endpoint.wait_idle().await;
    }
}

pub async fn connect_relay(relay: &RelayArgs, app_id: &str) -> Result<RelayConnection> {
    let auth_token = match &relay.auth_token_file {
        Some(path) => Some(read_auth_token(path).await?),
        None => relay.auth_token.clone(),
    };
    let config = ClientConfig {
        port: 0,
        verify_certificate: !relay.insecure,
        authorization_token: auth_token.clone(),
        follow_go_away: true,
    };
    let endpoint = Endpoint::<QUIC>::create_client(&config)?;
    info!(relay = %relay.url, "connecting to relay");
    let connecting = endpoint.connect(relay.url.as_str()).await?;
    let session = Arc::new(connecting.await?);
    let auth_token_refresh = match (&relay.auth_token_file, auth_token) {
        (Some(path), Some(token)) => Some(AuthTokenRefreshTask::run(
            session.clone(),
            path.clone(),
            app_id.to_string(),
            token,
        )),
        _ => None,
    };
    Ok(RelayConnection {
        session,
        auth_token_refresh,
        endpoint,
    })
}

/// Reads the catalog's latest group with the Joining FETCH (offset 0) that
/// MSF §5 requires beside the SUBSCRIBE. Falls back to the subscription's
/// next group when the FETCH fails, e.g. before the first catalog was sent.
pub async fn read_catalog(session: &Session, subscription: &Subscription) -> Result<Bytes> {
    match fetch_current_group(session, subscription).await {
        Ok(objects) => {
            if let Some(latest) = objects
                .into_iter()
                .rev()
                .find(|object| !object.payload.is_empty())
            {
                return Ok(latest.payload);
            }
        }
        Err(error) => tracing::warn!(%error, "joining FETCH of the catalog failed"),
    }
    let mut updates = track_reader(session, subscription).await?;
    match updates.next_object().await? {
        Some(object) => Ok(object.payload),
        None => anyhow::bail!("catalog stream ended before any object"),
    }
}

pub async fn subscribe(
    session: &Session,
    namespace: &str,
    name: &str,
    filter_type: FilterType,
) -> Result<Subscription> {
    let option = SubscribeOption {
        subscriber_priority: SUBSCRIBER_PRIORITY,
        group_order: GroupOrder::Ascending,
        forward: true,
        filter_type,
    };
    session
        .subscriber()
        .subscribe(namespace.to_string(), name.to_string(), option)
        .await
        .with_context(|| format!("failed to subscribe {namespace}/{name}"))
}

/// Waits for the first data stream of `subscription`.
pub async fn track_reader(session: &Session, subscription: &Subscription) -> Result<TrackReader> {
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

/// Reads the group of the subscription's Largest Location, up to that
/// location, with a Relative Joining FETCH, Joining Start 0 (draft-14 §9.16.2).
async fn fetch_current_group(
    session: &Session,
    subscription: &Subscription,
) -> Result<Vec<Object>> {
    let mut subscriber = session.subscriber();
    let handle = subscriber
        .fetch_relative_joining(subscription, 0, FetchOption::default())
        .await?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    let mut objects = Vec::new();
    loop {
        match receiver.receive().await? {
            Fetch::Object(object) => objects.push(object),
            Fetch::End => return Ok(objects),
        }
    }
}
