use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use moqt::{RequestErrorCode, Session, SessionEvent};
use object_store::{ObjectStore, path::Path};
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    recorder::{BroadcastOptions, UploadTarget, default_output_path, record_broadcast},
    relay::RelayConnection,
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
        let mut status = lock(&self.status);
        if status.state == RoomState::Recording {
            status.state = RoomState::Stopping;
        }
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
    let connection =
        RelayConnection::connect(&config.relay, config.verify_certificate, None).await?;
    let result = follow_members(
        &connection.session,
        &config.namespace_prefix,
        &config.output_dir,
        &options,
        stop,
        status,
    )
    .await;
    connection.close().await;
    // The queues the options hold have to go before the upload task can end.
    drop(options);
    if let Some((task, _)) = upload_task {
        tracing::info!(prefix = %config.namespace_prefix, "waiting for the uploads to finish");
        task.finish().await;
    }
    result
}

/// Returns once `stop` turns true or the session closes, and every
/// participant's recording has ended.
async fn follow_members(
    session: &Arc<Session>,
    namespace_prefix: &str,
    output_dir: &std::path::Path,
    options: &Arc<BroadcastOptions>,
    mut stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<()> {
    session
        .subscriber()
        .subscribe_namespace(namespace_prefix.to_string())
        .await?;
    tracing::info!(prefix = namespace_prefix, "room recording started");
    let mut members: Vec<(String, JoinHandle<()>)> = Vec::new();
    loop {
        let event = tokio::select! {
            _ = stop.wait_for(|stop| *stop) => break,
            close = session.closed() => {
                tracing::warn!(%close, "room session closed");
                break;
            }
            event = session.receive_event() => event?,
        };
        match event {
            SessionEvent::PublishNamespace(handler) => {
                let namespace = handler.track_namespace.clone();
                handler.ok().await?;
                members.retain(|(_, handle)| !handle.is_finished());
                if members.iter().any(|(member, _)| *member == namespace) {
                    continue;
                }
                tracing::info!(%namespace, "recording a participant");
                let handle = record_member(
                    Arc::clone(session),
                    namespace.clone(),
                    output_dir,
                    Arc::clone(options),
                    stop.clone(),
                    Arc::clone(status),
                );
                members.push((namespace, handle));
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
    for (_, handle) in members {
        let _ = handle.await;
    }
    Ok(())
}

fn record_member(
    session: Arc<Session>,
    namespace: String,
    output_dir: &std::path::Path,
    options: Arc<BroadcastOptions>,
    stop: watch::Receiver<bool>,
    status: Arc<Mutex<RoomStatus>>,
) -> JoinHandle<()> {
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
    tokio::spawn(async move {
        let result = record_broadcast(&session, &namespace, path, &options, stop).await;
        let mut status = lock(&status);
        let member = &mut status.members[index];
        member.finished = true;
        if let Err(error) = result {
            tracing::warn!(%namespace, error = format!("{error:#}"), "participant recording failed");
            member.error = Some(format!("{error:#}"));
        }
    })
}

fn lock(status: &Mutex<RoomStatus>) -> std::sync::MutexGuard<'_, RoomStatus> {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
