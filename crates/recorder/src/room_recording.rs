use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use moqt::{MigratingSession, RequestErrorCode, SessionEvent};
use object_store::{ObjectStore, path::Path};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::Instant,
};

use crate::{
    recorder::{BroadcastOptions, UploadTarget, default_output_path, record_broadcast},
    relay,
    upload_task::UploadTask,
};

/// Where a room recording connects and what it records. Every broadcast whose
/// namespace starts with `namespace_prefix` is recorded on its own, from the
/// moment it is published until it ends or the recording stops.
pub(crate) struct RoomConfig {
    pub(crate) relay: String,
    pub(crate) verify_certificate: bool,
    pub(crate) namespace_prefix: String,
    pub(crate) output_dir: PathBuf,
    /// The store and the key prefix the room's objects go under.
    pub(crate) upload: Option<(Arc<dyn ObjectStore>, Path)>,
    pub(crate) options: BroadcastOptions,
    pub(crate) limits: RecordingLimits,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RoomState {
    Recording,
    Stopping,
    Finished,
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct MemberStatus {
    pub(crate) namespace: String,
    pub(crate) started_at_ms: u64,
    pub(crate) finished: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RoomStatus {
    pub(crate) state: RoomState,
    pub(crate) members: Vec<MemberStatus>,
}

/// Records a room on its own relay session: SUBSCRIBE_NAMESPACE reports each
/// participant's namespace as PUBLISH_NAMESPACE, and each one is recorded
/// with `record_broadcast`. A participant who leaves ends their tracks, which
/// ends their recording; one who joins again is recorded into new files.
pub(crate) struct RoomRecording {
    handle: JoinHandle<()>,
    stop_sender: watch::Sender<bool>,
    status: Arc<Mutex<RoomStatus>>,
}

impl RoomRecording {
    pub(crate) fn run(config: RoomConfig) -> Self {
        let (stop_sender, stop) = watch::channel(false);
        let status = Arc::new(Mutex::new(RoomStatus {
            state: RoomState::Recording,
            members: Vec::new(),
        }));
        let task_status = Arc::clone(&status);
        let handle = tokio::spawn(async move {
            let state = match record(config, stop, &task_status).await {
                Ok(()) => RoomState::Finished,
                Err(error) => {
                    tracing::error!(error = format!("{error:#}"), "room recording failed");
                    RoomState::Failed(format!("{error:#}"))
                }
            };
            lock(&task_status).state = state;
        });
        Self {
            handle,
            stop_sender,
            status,
        }
    }

    pub(crate) fn status(&self) -> RoomStatus {
        lock(&self.status).clone()
    }

    /// Asks every participant's recording to close its files; `finished`
    /// then waits for them and for the uploads.
    pub(crate) fn stop(&self) {
        stopping(&self.status);
        let _ = self.stop_sender.send(true);
    }

    pub(crate) async fn finished(&mut self) {
        if let Err(error) = (&mut self.handle).await {
            tracing::error!(%error, "room recording task failed");
        }
    }
}

impl Drop for RoomRecording {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn record(
    config: RoomConfig,
    stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<()> {
    tokio::fs::create_dir_all(&config.output_dir)
        .await
        .with_context(|| format!("cannot create {}", config.output_dir.display()))?;
    let upload_task = config
        .upload
        .map(|(store, prefix)| (UploadTask::run(store), prefix));
    let options = Arc::new(BroadcastOptions {
        upload: upload_task.as_ref().map(|(task, prefix)| UploadTarget {
            queue: task.queue(),
            prefix: prefix.clone(),
        }),
        ..config.options
    });
    let session = Arc::new(relay::connect(&config.relay, config.verify_certificate, None).await?);
    let result = follow_members(
        &session,
        &config.namespace_prefix,
        &config.output_dir,
        config.limits,
        &options,
        stop,
        status,
    )
    .await;
    session.close().await;
    // The queues the options hold have to go before the upload task can end.
    drop(options);
    if let Some((task, _)) = upload_task {
        tracing::info!(prefix = %config.namespace_prefix, "waiting for the uploads to finish");
        task.finish().await;
    }
    result
}

/// Returns once `stop` turns true, the room stops by itself (see
/// `AutoStop`) or the session closes, and every participant's recording has
/// ended.
async fn follow_members(
    session: &Arc<MigratingSession>,
    namespace_prefix: &str,
    output_dir: &std::path::Path,
    limits: RecordingLimits,
    options: &Arc<BroadcastOptions>,
    mut stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<()> {
    session
        .subscribe_namespace(namespace_prefix.to_string())
        .await?;
    tracing::info!(prefix = namespace_prefix, "room recording started");
    let (member_stop_sender, member_stop) = watch::channel(false);
    let mut members: JoinSet<String> = JoinSet::new();
    let mut recorded: HashSet<String> = HashSet::new();
    let mut auto_stop = AutoStop::new(Instant::now(), limits);
    loop {
        let (deadline, reason) = auto_stop.next();
        let event = tokio::select! {
            _ = stop.wait_for(|stop| *stop) => break,
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!(prefix = namespace_prefix, reason, "stopping the room recording");
                stopping(status);
                break;
            }
            close = session.closed() => {
                tracing::warn!(%close, "room session closed");
                break;
            }
            Some(finished) = members.join_next(), if !members.is_empty() => {
                if let Ok(namespace) = finished {
                    recorded.remove(&namespace);
                }
                auto_stop.members_changed(recorded.len(), Instant::now());
                continue;
            }
            event = session.receive_event() => event?,
        };
        match event {
            SessionEvent::PublishNamespace(handler) => {
                let namespace = handler.track_namespace.clone();
                handler.ok().await?;
                if !recorded.insert(namespace.clone()) {
                    continue;
                }
                tracing::info!(%namespace, "recording a participant");
                members.spawn(record_member(
                    Arc::clone(session),
                    namespace,
                    output_dir,
                    Arc::clone(options),
                    member_stop.clone(),
                    Arc::clone(status),
                ));
                auto_stop.members_changed(recorded.len(), Instant::now());
            }
            // Accepting forwarded PUBLISHes would deliver every track the
            // participant publishes, not only the selected ones.
            SessionEvent::Publish(handler) => {
                handler
                    .error(
                        RequestErrorCode::Uninterested,
                        "the recorder subscribes to the tracks it records".to_string(),
                    )
                    .await?;
            }
            SessionEvent::PublishNamespaceDone(handler) => {
                tracing::info!(namespace = %handler.track_namespace, "participant left");
            }
            SessionEvent::Disconnected() => break,
            event => tracing::debug!(?event, "room session event ignored"),
        }
    }
    let _ = member_stop_sender.send(true);
    members.join_all().await;
    Ok(())
}

/// How long a room recording may run by itself.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordingLimits {
    pub(crate) max_duration: Duration,
    /// Stop once no participant has been recorded for this long, counted
    /// from the start too.
    pub(crate) empty_room_timeout: Duration,
}

/// The moment a room recording stops by itself.
struct AutoStop {
    max_end: Instant,
    empty_room_timeout: Duration,
    /// `None` while some participant is recorded.
    empty_end: Option<Instant>,
}

impl AutoStop {
    fn new(now: Instant, limits: RecordingLimits) -> Self {
        Self {
            max_end: now + limits.max_duration,
            empty_room_timeout: limits.empty_room_timeout,
            empty_end: Some(now + limits.empty_room_timeout),
        }
    }

    fn members_changed(&mut self, recorded: usize, now: Instant) {
        self.empty_end = match (recorded, self.empty_end) {
            (0, Some(end)) => Some(end),
            (0, None) => Some(now + self.empty_room_timeout),
            _ => None,
        };
    }

    fn next(&self) -> (Instant, &'static str) {
        match self.empty_end {
            Some(end) if end < self.max_end => (end, "the room has been empty"),
            _ => (self.max_end, "the recording reached its maximum length"),
        }
    }
}

fn stopping(status: &Mutex<RoomStatus>) {
    let mut status = lock(status);
    if status.state == RoomState::Recording {
        status.state = RoomState::Stopping;
    }
}

fn record_member(
    session: Arc<MigratingSession>,
    namespace: String,
    output_dir: &std::path::Path,
    options: Arc<BroadcastOptions>,
    stop: watch::Receiver<bool>,
    status: Arc<Mutex<RoomStatus>>,
) -> impl Future<Output = String> + use<> {
    let now = SystemTime::now();
    let index = {
        let mut status = lock(&status);
        status.members.push(MemberStatus {
            namespace: namespace.clone(),
            started_at_ms: now
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            finished: false,
            error: None,
        });
        status.members.len() - 1
    };
    let path = output_dir.join(default_output_path(&namespace, now));
    async move {
        let result = record_broadcast(&session, &namespace, path, &options, stop).await;
        let mut status = lock(&status);
        let member = &mut status.members[index];
        member.finished = true;
        if let Err(error) = result {
            tracing::warn!(%namespace, error = format!("{error:#}"), "participant recording failed");
            member.error = Some(format!("{error:#}"));
        }
        namespace
    }
}

fn lock(status: &Mutex<RoomStatus>) -> std::sync::MutexGuard<'_, RoomStatus> {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: RecordingLimits = RecordingLimits {
        max_duration: Duration::from_secs(7200),
        empty_room_timeout: Duration::from_secs(300),
    };

    #[test]
    fn an_empty_room_stops_five_minutes_after_the_start() {
        // Arrange
        let start = Instant::now();

        // Act
        let auto_stop = AutoStop::new(start, LIMITS);

        // Assert
        assert_eq!(auto_stop.next().0, start + Duration::from_secs(300));
    }

    #[test]
    fn the_empty_room_timer_runs_only_while_nobody_is_recorded() {
        // Arrange
        let start = Instant::now();
        let mut auto_stop = AutoStop::new(start, LIMITS);
        let joined = start + Duration::from_secs(60);
        let left = start + Duration::from_secs(600);

        // Act
        auto_stop.members_changed(1, joined);
        let while_recorded = auto_stop.next().0;
        auto_stop.members_changed(0, left);
        auto_stop.members_changed(0, left + Duration::from_secs(10));

        // Assert: a second empty report keeps the first deadline
        assert_eq!(while_recorded, start + Duration::from_secs(7200));
        assert_eq!(auto_stop.next().0, left + Duration::from_secs(300));
    }

    #[test]
    fn the_maximum_length_wins_over_a_later_empty_room_deadline() {
        // Arrange
        let start = Instant::now();
        let mut auto_stop = AutoStop::new(start, LIMITS);

        auto_stop.members_changed(1, start + Duration::from_secs(10));

        // Act: the last participant leaves 100 s before the maximum length
        auto_stop.members_changed(0, start + Duration::from_secs(7100));

        // Assert
        assert_eq!(
            auto_stop.next(),
            (
                start + Duration::from_secs(7200),
                "the recording reached its maximum length"
            )
        );
    }
}
