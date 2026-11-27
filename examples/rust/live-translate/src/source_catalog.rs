use std::time::Duration;

use anyhow::{Context, Result};
use moq_wasm::{
    Session,
    media::{Catalog, Catalogs, RemoteBroadcast},
};
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
        session: &Session,
        source: RemoteBroadcast,
        catalog_sender: mpsc::UnboundedSender<Catalog>,
    ) -> Self {
        let join_handle = tokio::spawn(session.scoped(async move {
            if let Err(error) = follow(&source, &catalog_sender).await {
                let track_namespace = source.namespace().join("/");
                tracing::warn!(track_namespace, ?error, "source catalog ended");
            }
        }));
        Self { join_handle }
    }
}

impl Drop for SourceCatalog {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

async fn follow(
    source: &RemoteBroadcast,
    catalog_sender: &mpsc::UnboundedSender<Catalog>,
) -> Result<()> {
    let mut catalogs = subscribe_with_retry(source).await?;
    while let Some(catalog) = catalogs.next().await? {
        if catalog_sender.send(catalog).is_err() {
            break;
        }
    }
    Ok(())
}

/// The catalog track is published a moment after the namespace, so the first
/// SUBSCRIBE may be refused.
async fn subscribe_with_retry(source: &RemoteBroadcast) -> Result<Catalogs> {
    let mut attempt = 1;
    loop {
        match source.catalogs().await {
            Ok(catalogs) => return Ok(catalogs),
            Err(error) if attempt < SUBSCRIBE_ATTEMPTS => {
                let track_namespace = source.namespace().join("/");
                tracing::info!(
                    track_namespace,
                    attempt,
                    ?error,
                    "source catalog not ready, retrying"
                );
                attempt += 1;
                tokio::time::sleep(SUBSCRIBE_RETRY_DELAY).await;
            }
            Err(error) => return Err(error).context("subscribe to the source catalog"),
        }
    }
}
