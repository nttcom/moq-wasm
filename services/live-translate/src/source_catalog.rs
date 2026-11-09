use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{Session, Subscriber, Subscription, TrackReader};
use msf::{CATALOG_TRACK_NAME, Catalog};
use tokio::{sync::mpsc, task::JoinHandle};

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
    let (subscription, mut reader) = subscribe_with_retry(&mut subscriber, namespace).await?;
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
) -> Result<(Subscription, TrackReader)> {
    let mut attempt = 1;
    loop {
        match subscriber
            .subscribe_current_group(namespace.to_string(), CATALOG_TRACK_NAME.to_string())
            .await
        {
            Ok(subscribed) => return Ok(subscribed),
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
