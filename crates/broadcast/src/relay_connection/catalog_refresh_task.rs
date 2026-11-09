use std::{sync::Arc, time::Duration};

use moqt::executor::{self, JoinHandle};

use super::catalog_track::CatalogTrack;

/// Sends of a namespace's catalog are at most two ticks (30 s) apart, half the
/// relay's default RELAY_CACHE_TTL_SECS (draft-ietf-moq-msf-01 §5): a tick
/// resends a catalog only when none was sent since the previous tick.
const CATALOG_REFRESH_TICK: Duration = Duration::from_secs(15);

/// Resends catalogs on a timer, whether or not media flows, and resends every
/// sent catalog when the session moves to the relay named in a GOAWAY, since
/// that relay has no catalog until the next group.
pub(super) struct CatalogRefreshTask {
    join_handle: JoinHandle,
}

impl CatalogRefreshTask {
    pub(super) fn run(catalog: Arc<CatalogTrack>) -> Self {
        Self {
            join_handle: executor::spawn("broadcast-catalog-refresh", Self::refresh(catalog)),
        }
    }

    async fn refresh(catalog: Arc<CatalogTrack>) {
        let mut generation = catalog.session.connection_generation();
        let mut may_move = true;
        loop {
            let namespaces = tokio::select! {
                _ = executor::sleep(CATALOG_REFRESH_TICK) => {
                    catalog.state.lock().await.take_catalogs_to_refresh()
                }
                moved = catalog.session.moved_past(generation), if may_move => {
                    if !moved {
                        may_move = false;
                        continue;
                    }
                    generation = catalog.session.connection_generation();
                    catalog.state.lock().await.sent_catalogs()
                }
            };
            for namespace_path in namespaces {
                if let Err(error) = catalog.send(&namespace_path).await {
                    if catalog.state.lock().await.disconnected {
                        tracing::debug!(%error, "catalog refresh ended with the session");
                    } else {
                        tracing::warn!(%error, track_namespace = %namespace_path, "catalog refresh stopped");
                    }
                    return;
                }
            }
        }
    }
}

impl Drop for CatalogRefreshTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
