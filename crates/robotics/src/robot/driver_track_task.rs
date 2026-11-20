use moqt::client::Subscription;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    record::{Action, decode_object},
    tracks::TrackKind,
};

/// An action read from a candidate's `action` track.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DriverRecord {
    /// The candidate's namespace relative to the room.
    pub(super) sender: String,
    pub(super) action: Action,
}

/// What one `DriverTrackTask` reads.
pub(super) struct DriverTrack {
    /// The subscription that started with the candidate's PUBLISH of its
    /// `action` track.
    pub(super) subscription: Subscription,
    pub(super) namespace: Vec<String>,
    pub(super) sender: String,
    pub(super) record_sender: mpsc::UnboundedSender<DriverRecord>,
}

/// Reads one PUBLISH of a candidate's `action` track until its subscription
/// ends. Each record is a group on its own stream, so groups may arrive out
/// of order; one not after the last read is stale and is not passed on.
pub(super) struct DriverTrackTask {
    handle: JoinHandle<()>,
}

impl DriverTrackTask {
    pub(super) fn run(track: DriverTrack) -> Self {
        Self {
            handle: tokio::spawn(track.read()),
        }
    }
}

impl Drop for DriverTrackTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl DriverTrack {
    async fn read(self) {
        let done = self.subscription.done();
        tokio::pin!(done);
        let mut last_read = None;
        loop {
            let object = tokio::select! {
                object = self.subscription.next() => object,
                _ = &mut done => return,
            };
            let object = match object {
                Ok(Some(object)) => object,
                Ok(None) => return,
                Err(error) => {
                    tracing::debug!(%error, track_namespace = %self.namespace.join("/"), track_name = TrackKind::Action.name(), "driver track ended");
                    return;
                }
            };
            let location = (object.group_id, object.object_id);
            if last_read.is_some_and(|last| location <= last) {
                continue;
            }
            last_read = Some(location);
            for action in self.decode(&object.payload) {
                let record = DriverRecord {
                    sender: self.sender.clone(),
                    action,
                };
                if self.record_sender.send(record).is_err() {
                    return;
                }
            }
        }
    }

    fn decode(&self, payload: &[u8]) -> Vec<Action> {
        match decode_object::<Action>(payload) {
            Ok(records) => records.into_iter().map(|record| record.data).collect(),
            Err(error) => {
                tracing::warn!(%error, track_namespace = %self.namespace.join("/"), track_name = TrackKind::Action.name(), "malformed driver record dropped");
                Vec::new()
            }
        }
    }
}
