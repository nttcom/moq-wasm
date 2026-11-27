use std::time::Duration;

use bytes::Bytes;
use moq_wasm::{Session, TrackWriter};
use tokio::{sync::watch, task::JoinHandle};

const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Writes the translated catalog as a group of its own at once, every
/// `CATALOG_REFRESH_INTERVAL` so the relay keeps a copy, on each `update`, and
/// for each new subscriber, since a SUBSCRIBE starts after the objects
/// already written.
pub(crate) struct CatalogTrack {
    join_handle: JoinHandle<()>,
    payload_sender: watch::Sender<Bytes>,
}

impl CatalogTrack {
    pub(crate) fn run(session: &Session, writer: TrackWriter, payload: Bytes) -> Self {
        let (payload_sender, mut payload_receiver) = watch::channel(payload);
        let join_handle = tokio::spawn(session.scoped(async move {
            let mut refresh = tokio::time::interval(CATALOG_REFRESH_INTERVAL);
            loop {
                tokio::select! {
                    _ = refresh.tick() => {}
                    changed = payload_receiver.changed() => if changed.is_err() {
                        return;
                    },
                    subscribed = writer.next_subscriber() => if !subscribed {
                        return;
                    },
                }
                let payload = payload_receiver.borrow_and_update().clone();
                if let Err(error) = writer.write_group(payload).await {
                    tracing::warn!(?error, "catalog track stopped");
                    return;
                }
            }
        }));
        Self {
            join_handle,
            payload_sender,
        }
    }

    /// Returns whether `payload` differs from the catalog being written.
    pub(crate) fn update(&self, payload: Bytes) -> bool {
        self.payload_sender.send_if_modified(|current| {
            let changed = *current != payload;
            if changed {
                *current = payload;
            }
            changed
        })
    }
}

impl Drop for CatalogTrack {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}
