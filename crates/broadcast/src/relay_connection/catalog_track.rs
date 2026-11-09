use std::sync::Arc;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use moqt::{
    Session,
    publish::{Origin, WriteOutcome},
};
use msf::CATALOG_TRACK_NAME;
use tokio::sync::Mutex;

use super::{RelayState, TrackKey, ensure_origin_track};
use crate::{
    broadcast::{build_catalog, is_cmaf},
    stream_ledger::StreamLedger,
};

/// Writes a namespace's catalog as a new group, when the catalog changes and
/// when `CatalogRefreshTask` resends it.
pub(super) struct CatalogTrack {
    pub(super) session: Arc<Session>,
    origin: Origin,
    pub(super) state: Arc<Mutex<RelayState>>,
    ledger: StreamLedger,
    sending: Mutex<()>,
}

impl CatalogTrack {
    pub(super) fn new(
        session: Arc<Session>,
        origin: Origin,
        state: Arc<Mutex<RelayState>>,
        ledger: StreamLedger,
    ) -> Self {
        Self {
            session,
            origin,
            state,
            ledger,
            sending: Mutex::new(()),
        }
    }

    /// Publishes the non-CMAF tracks and the catalog track, then writes the
    /// catalog as a new group. The caller announces the namespace first, since
    /// the relay routes a PUBLISH by its namespace. The non-CMAF tracks are
    /// published before the catalog is sent, so a viewer subscribing to a
    /// track it read there finds the relay already ingesting it; the relay
    /// would otherwise forward the SUBSCRIBE here. CMAF tracks wait for that
    /// SUBSCRIBE, so the uplink carries them only while someone watches them.
    ///
    /// Sends do not overlap: `Origin::write_group` does not send a group
    /// placed while another write on the track is in flight, and a snapshot
    /// built earlier could otherwise be written last.
    pub(super) async fn send(&self, namespace_path: &str) -> Result<()> {
        let _sending = self.sending.lock().await;
        let catalog = {
            let guard = self.state.lock().await;
            if guard.disconnected {
                bail!("MoQ publisher disconnected");
            }
            let metadata = guard
                .catalogs
                .get(namespace_path)
                .cloned()
                .unwrap_or_default();
            build_catalog(namespace_path, &metadata)?
        };
        let published_tracks = catalog
            .tracks
            .iter()
            .flatten()
            .filter(|track| !is_cmaf(track))
            .map(|track| track.name.as_str());
        for track_name in published_tracks.chain([CATALOG_TRACK_NAME]) {
            self.publish_track((namespace_path.to_string(), track_name.to_string()))
                .await?;
        }
        let payload = Bytes::from(serde_json::to_vec(&catalog).context("serialize msf catalog")?);
        tracing::debug!(track_namespace = %namespace_path, catalog = %String::from_utf8_lossy(&payload), "catalog snapshot");
        self.state
            .lock()
            .await
            .catalog_sent_since_tick
            .insert(namespace_path.to_string(), true);
        let bytes = payload.len();
        match self
            .origin
            .write_group(namespace_path, CATALOG_TRACK_NAME, payload)
            .await
            .context("send catalog group")?
        {
            WriteOutcome::Sent(placement) => {
                self.ledger
                    .open(CATALOG_TRACK_NAME, placement.location.group_id);
                self.ledger.add_object(CATALOG_TRACK_NAME, bytes);
                self.ledger.finish(CATALOG_TRACK_NAME);
                tracing::info!(track_namespace = %namespace_path, group_id = placement.location.group_id, "catalog sent");
            }
            WriteOutcome::Stopped(placement) => {
                self.ledger
                    .open(CATALOG_TRACK_NAME, placement.location.group_id);
                self.ledger.finish(CATALOG_TRACK_NAME);
                tracing::info!(track_namespace = %namespace_path, "subscriber stopped the catalog group");
            }
            WriteOutcome::NotSent => {}
        }
        Ok(())
    }

    async fn publish_track(&self, key: TrackKey) -> Result<()> {
        ensure_origin_track(&self.origin, &key).await;
        self.state
            .lock()
            .await
            .published_tracks
            .entry(key.0.clone())
            .or_default()
            .insert(key.1.clone());
        self.origin.publish(&self.session, &key.0, &key.1).await
    }
}
