use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use broadcast::Broadcast;
use moqt::publish::GroupBoundary;

use crate::{
    record::Action,
    tracks::{TrackKind, write_record},
};

/// The driver side of a robot room
/// (`docs/architecture/robotics/architecture.md`, Driving and holding
/// still): it publishes `action` on a driver's broadcast, an operator's
/// `<room>/operators/<id>` or the policy's `<room>/bots/policy`, each record
/// a group of its own.
pub struct Driver {
    broadcast: Broadcast,
    next_seq: AtomicU64,
}

impl Driver {
    /// Lists `action` in `broadcast`'s catalog at its priority. Fails when
    /// the track cannot be offered, as once the relay session has ended.
    pub async fn offer(broadcast: &Broadcast) -> Result<Self> {
        let namespace = broadcast.namespace().join("/");
        broadcast
            .offer_event_timeline(
                TrackKind::Action.catalog_track(Some(namespace)),
                TrackKind::Action.priority(),
            )
            .await
            .context("offer action")?;
        Ok(Self {
            broadcast: broadcast.clone(),
            next_seq: AtomicU64::new(1),
        })
    }

    /// Sends a chunk answering the robot's observation of robot time
    /// `basis_ms` (the `T` of its newest `state`): entry `i` is meant for
    /// `basis_ms + (i + 1) × dt_ms`. Fails once the relay session has ended.
    pub async fn send(&self, basis_ms: u64, dt_ms: u64, actions: Vec<Vec<f64>>) -> Result<()> {
        let action = Action {
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            basis: basis_ms,
            dt_ms,
            actions,
        };
        write_record(
            &self.broadcast,
            TrackKind::Action,
            GroupBoundary::Next,
            action,
        )
        .await
    }
}
