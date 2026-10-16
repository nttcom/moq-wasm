use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{MigratingSession, MigratingTrack, subscribe::FilterType};
use msf::Catalog;

use crate::relay;

/// Yields the catalog's current object from the joining FETCH
/// (draft-ietf-moq-msf-01 §5), then later objects from the subscription,
/// whose first stream arrives with the next catalog object, up to 30 s later.
pub(crate) struct CatalogReader {
    track: MigratingTrack,
    current: Option<Bytes>,
}

impl CatalogReader {
    pub(crate) async fn open(session: &MigratingSession, namespace: &str) -> Result<Self> {
        let track = relay::subscribe(
            session,
            namespace,
            msf::CATALOG_TRACK_NAME,
            FilterType::NextGroupStart,
        )
        .await?;
        let current = match relay::fetch_joining_group(&track).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(%error, "joining FETCH of the catalog failed");
                None
            }
        };
        Ok(Self { track, current })
    }

    pub(crate) async fn next(&mut self) -> Result<Catalog> {
        let payload = match self.current.take() {
            Some(payload) => payload,
            None => {
                self.track
                    .next_object()
                    .await?
                    .context("catalog track ended")?
                    .payload
            }
        };
        serde_json::from_slice(&payload).context("catalog is not MSF JSON")
    }
}
