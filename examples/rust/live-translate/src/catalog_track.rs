use std::time::Duration;

use bytes::Bytes;
use moqt::TrackWriter;
use tokio::{sync::mpsc, task::JoinHandle};

const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// One catalog subscriber's track: the catalog goes out at once, again every
/// `CATALOG_REFRESH_INTERVAL` so the relay keeps a copy, and whenever
/// `update` replaces it.
pub(crate) struct CatalogTrack {
    join_handle: JoinHandle<()>,
    payload_sender: mpsc::UnboundedSender<Bytes>,
}

impl CatalogTrack {
    pub(crate) fn run(mut writer: TrackWriter, mut payload: Bytes) -> Self {
        let (payload_sender, mut payload_receiver) = mpsc::unbounded_channel::<Bytes>();
        let join_handle = tokio::spawn(async move {
            let mut refresh = tokio::time::interval(CATALOG_REFRESH_INTERVAL);
            loop {
                tokio::select! {
                    _ = refresh.tick() => {}
                    updated = payload_receiver.recv() => match updated {
                        Some(updated) => payload = updated,
                        None => return,
                    },
                }
                if let Err(error) = writer.write_group(payload.clone()).await {
                    tracing::warn!(?error, "catalog track stopped");
                    return;
                }
            }
        });
        Self {
            join_handle,
            payload_sender,
        }
    }

    pub(crate) fn update(&self, payload: Bytes) {
        let _ = self.payload_sender.send(payload);
    }
}

impl Drop for CatalogTrack {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
