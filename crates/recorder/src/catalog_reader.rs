use anyhow::{Context, Result};
use bytes::Bytes;
use moqt::{Session, Subscription, TrackReader, subscribe::FilterType};
use msf::Catalog;

use crate::relay;

/// Yields the catalog's current object from the joining FETCH
/// (draft-ietf-moq-msf-01 §5), then later objects from the subscription. The
/// subscription reader opens only after the FETCH: its first stream arrives
/// with the next catalog object, up to 30 s later.
pub(crate) struct CatalogReader<'a> {
    session: &'a Session,
    subscription: Subscription,
    current: Option<Bytes>,
    reader: Option<TrackReader>,
}

impl<'a> CatalogReader<'a> {
    pub(crate) async fn open(session: &'a Session, namespace: &str) -> Result<Self> {
        let subscription = relay::subscribe(
            session,
            namespace,
            msf::CATALOG_TRACK_NAME,
            FilterType::NextGroupStart,
        )
        .await?;
        let current = match relay::fetch_joining_group(session, &subscription).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(%error, "joining FETCH of the catalog failed");
                None
            }
        };
        Ok(Self {
            session,
            subscription,
            current,
            reader: None,
        })
    }

    pub(crate) async fn next(&mut self) -> Result<Catalog> {
        let payload = match self.current.take() {
            Some(payload) => payload,
            None => {
                let reader = match &mut self.reader {
                    Some(reader) => reader,
                    None => self
                        .reader
                        .insert(relay::track_reader(self.session, &self.subscription).await?),
                };
                reader
                    .next_object()
                    .await?
                    .context("catalog track ended")?
                    .payload
            }
        };
        serde_json::from_slice(&payload).context("catalog is not MSF JSON")
    }
}
