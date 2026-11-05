use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use moqt::{
    Session, Subscriber, Subscription, TrackReader,
    fetch::{Fetch, FetchOption},
    object::{ContentExists, DataReceiver},
    subscribe::SubscribeOption,
};
use msf::Catalog;
use tokio::{sync::mpsc, task::JoinHandle};

pub(crate) const CATALOG_TRACK: &str = "catalog";
const SUBSCRIBE_ATTEMPTS: u32 = 5;
const SUBSCRIBE_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Follows the source broadcast's catalog: the one the relay caches when the
/// subscription starts, then every one the source publishes after it. The
/// browser publisher lists each track only once its encoder runs, so the
/// first catalog may lack the video or the audio.
pub(crate) struct SourceCatalog {
    join_handle: JoinHandle<()>,
}

impl SourceCatalog {
    pub(crate) fn run(
        session: Arc<Session>,
        namespace: String,
        catalog_sender: mpsc::UnboundedSender<Catalog>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            if let Err(error) = follow(&session, &namespace, &catalog_sender).await {
                tracing::warn!(%namespace, ?error, "source catalog ended");
            }
        });
        Self { join_handle }
    }
}

impl Drop for SourceCatalog {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn follow(
    session: &Session,
    namespace: &str,
    catalog_sender: &mpsc::UnboundedSender<Catalog>,
) -> Result<()> {
    let mut subscriber = session.subscriber();
    let subscription = subscribe_with_retry(&mut subscriber, namespace).await?;
    if let ContentExists::True { .. } = subscription.content_exists() {
        let payload = cached_catalog_object(&mut subscriber, &subscription).await?;
        deliver(catalog_sender, &payload);
    }
    let DataReceiver::Stream(factory) = subscriber.accept_data_receiver(&subscription).await?
    else {
        bail!("source catalog arrived as datagrams");
    };
    let mut reader = TrackReader::new(factory);
    while let Some(object) = reader.next_object().await? {
        if catalog_sender.is_closed() {
            break;
        }
        deliver(catalog_sender, &object.payload);
    }
    let _ = subscriber.unsubscribe(&subscription).await;
    Ok(())
}

/// The catalog track is published a moment after the namespace, so the first
/// SUBSCRIBE may be refused.
async fn subscribe_with_retry(
    subscriber: &mut Subscriber,
    namespace: &str,
) -> Result<Subscription> {
    let mut attempt = 1;
    loop {
        match subscriber
            .subscribe(
                namespace.to_string(),
                CATALOG_TRACK.to_string(),
                SubscribeOption::default(),
            )
            .await
        {
            Ok(subscription) => return Ok(subscription),
            Err(error) if attempt < SUBSCRIBE_ATTEMPTS => {
                tracing::info!(%namespace, attempt, ?error, "source catalog not ready, retrying");
                attempt += 1;
                tokio::time::sleep(SUBSCRIBE_RETRY_DELAY).await;
            }
            Err(error) => return Err(error).context("subscribe to the source catalog"),
        }
    }
}

fn deliver(catalog_sender: &mpsc::UnboundedSender<Catalog>, payload: &Bytes) {
    match serde_json::from_slice::<Catalog>(payload) {
        Ok(catalog) => {
            let _ = catalog_sender.send(catalog);
        }
        Err(error) => tracing::warn!(?error, "source catalog object is not an MSF catalog"),
    }
}

async fn cached_catalog_object(
    subscriber: &mut Subscriber,
    subscription: &Subscription,
) -> Result<Bytes> {
    let handle = subscriber
        .fetch_relative_joining(subscription, 0, FetchOption::default())
        .await
        .context("fetch the cached source catalog")?;
    let mut receiver = subscriber.accept_fetch_receiver(&handle).await?;
    loop {
        match receiver.receive().await? {
            Fetch::Object(object) => {
                if !object.payload.is_empty() {
                    return Ok(object.payload);
                }
            }
            Fetch::End => bail!("cached source catalog was empty"),
        }
    }
}
