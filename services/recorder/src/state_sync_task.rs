use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};

use crate::{
    room_recording::{RoomStatus, lock, stopping},
    room_state::RoomRecord,
    store::RecordingStore,
};

/// How often the job looks for a stop request and writes changed progress.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How often the state is written even when nothing changed, so that the
/// API can tell a running job from one that died.
pub(crate) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Keeps a recording job's `state/<id>.json` in sync with its `RoomStatus`
/// and turns a stop request in the store, SIGTERM (Cloud Run stopping the
/// job) or SIGINT into `stop_sender`.
pub(crate) struct StateSyncTask {
    handle: JoinHandle<()>,
    finish_sender: Option<oneshot::Sender<()>>,
}

impl StateSyncTask {
    pub(crate) fn run(
        store: RecordingStore,
        record: RoomRecord,
        status: Arc<Mutex<RoomStatus>>,
        stop_sender: watch::Sender<bool>,
    ) -> Result<Self> {
        let mut terminate = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        let (finish_sender, mut finish_receiver) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let mut poll = tokio::time::interval(POLL_INTERVAL);
            poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
            let mut written: Option<(RoomRecord, Instant)> = None;
            loop {
                tokio::select! {
                    _ = &mut finish_receiver => break,
                    _ = poll.tick() => {}
                    _ = terminate.recv() => request_stop(&status, &stop_sender, "SIGTERM"),
                    _ = interrupt.recv() => request_stop(&status, &stop_sender, "SIGINT"),
                }
                if !*stop_sender.borrow() {
                    match store.stop_requested(&record.id).await {
                        Ok(true) => request_stop(&status, &stop_sender, "the API"),
                        Ok(false) => {}
                        Err(error) => tracing::warn!(
                            error = format!("{error:#}"),
                            "cannot read the stop request"
                        ),
                    }
                }
                let current = with_status(&record, &lock(&status));
                let due = written.as_ref().is_none_or(|(last, at)| {
                    !same_progress(last, &current) || at.elapsed() >= HEARTBEAT_INTERVAL
                });
                if !due {
                    continue;
                }
                let current = RoomRecord {
                    updated_at_ms: now_ms(),
                    ..current
                };
                match store.write_state(&current).await {
                    Ok(()) => written = Some((current, Instant::now())),
                    Err(error) => tracing::warn!(
                        error = format!("{error:#}"),
                        "cannot write the recording state"
                    ),
                }
            }
        });
        Ok(Self {
            handle,
            finish_sender: Some(finish_sender),
        })
    }

    /// Stops syncing; the caller writes the final state afterwards.
    pub(crate) async fn finish(mut self) {
        if let Some(sender) = self.finish_sender.take() {
            let _ = sender.send(());
        }
        if let Err(error) = (&mut self.handle).await {
            tracing::error!(%error, "state sync task failed");
        }
    }
}

impl Drop for StateSyncTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

pub(crate) fn with_status(record: &RoomRecord, status: &RoomStatus) -> RoomRecord {
    RoomRecord {
        state: status.state.clone(),
        members: status.members.clone(),
        ..record.clone()
    }
}

pub(crate) fn now_ms() -> u64 {
    moqt::executor::system_time().as_millis() as u64
}

fn same_progress(a: &RoomRecord, b: &RoomRecord) -> bool {
    a.state == b.state && a.members == b.members
}

fn request_stop(status: &Mutex<RoomStatus>, stop_sender: &watch::Sender<bool>, by: &str) {
    if !*stop_sender.borrow() {
        tracing::info!(by, "stopping the room recording");
        stopping(status);
        let _ = stop_sender.send(true);
    }
}
