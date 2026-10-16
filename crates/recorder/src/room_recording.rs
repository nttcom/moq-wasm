use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use moqt::{RequestErrorCode, Session, SessionEvent};
use object_store::{ObjectStore, path::Path};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::Instant,
};

use crate::{
    composite::{CompositeQueue, MemberFiles, composite_room},
    recorder::{BroadcastOptions, UploadTarget, default_output_path, record_broadcast},
    recording::RecordedFile,
    relay::RelayConnection,
    upload_task::{Upload, UploadTask},
};

const COMPOSITE_FILE_NAME: &str = "composite.mp4";

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
    /// Composite the room into one MP4 once the recording has ended.
    pub(crate) composite: Option<CompositeQueue>,
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompositeState {
    /// While the room is recorded and while another room is composited.
    Waiting,
    Composing,
    Done,
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RoomStatus {
    pub(crate) state: RoomState,
    pub(crate) members: Vec<MemberStatus>,
    pub(crate) composite: Option<CompositeState>,
}

/// Records a room on its own relay session: SUBSCRIBE_NAMESPACE reports each
/// participant's namespace as PUBLISH_NAMESPACE, and each one is recorded
/// with `record_broadcast`. A participant who leaves ends their tracks, which
/// ends their recording; one who joins again is recorded into new files.
pub(crate) struct RoomRecording {
    handle: JoinHandle<()>,
    stop_sender: watch::Sender<bool>,
    cancel_sender: watch::Sender<bool>,
    status: Arc<Mutex<RoomStatus>>,
}

impl RoomRecording {
    pub(crate) fn run(config: RoomConfig) -> Self {
        let (stop_sender, stop) = watch::channel(false);
        let (cancel_sender, cancel) = watch::channel(false);
        let status = Arc::new(Mutex::new(RoomStatus {
            state: RoomState::Recording,
            members: Vec::new(),
            composite: config.composite.as_ref().map(|_| CompositeState::Waiting),
        }));
        let task_status = Arc::clone(&status);
        let handle = tokio::spawn(async move {
            let composite = config.composite.clone();
            let output_dir = config.output_dir.clone();
            let upload = config.upload.clone();
            match record(config, stop, &task_status).await {
                Ok(room) => {
                    lock(&task_status).state = RoomState::Finished;
                    if let Some(queue) = composite {
                        let job = CompositeJob {
                            queue,
                            output_dir,
                            upload,
                            cancel,
                        };
                        let state = job.run(room, &task_status).await;
                        lock(&task_status).composite = Some(state);
                    }
                }
                Err(error) => {
                    tracing::error!(error = format!("{error:#}"), "room recording failed");
                    let mut status = lock(&task_status);
                    status.state = RoomState::Failed(format!("{error:#}"));
                    if status.composite.is_some() {
                        status.composite =
                            Some(CompositeState::Skipped("the recording failed".to_string()));
                    }
                }
            }
        });
        Self {
            handle,
            stop_sender,
            cancel_sender,
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

    /// Gives up the composite this recording waits for or is making.
    pub(crate) fn cancel_composite(&self) {
        let _ = self.cancel_sender.send(true);
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

struct RecordedRoom {
    members: Vec<MemberFiles>,
    failed_uploads: Vec<PathBuf>,
}

async fn record(
    config: RoomConfig,
    stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<RecordedRoom> {
    tokio::fs::create_dir_all(&config.output_dir)
        .await
        .with_context(|| format!("cannot create {}", config.output_dir.display()))?;
    let upload_task = config
        .upload
        .map(|(store, prefix)| (UploadTask::run(store, config.composite.is_some()), prefix));
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
        config.limits,
        &options,
        stop,
        status,
    )
    .await;
    connection.close().await;
    // The queues the options hold have to go before the upload task can end.
    drop(options);
    let mut failed_uploads = Vec::new();
    if let Some((task, _)) = upload_task {
        tracing::info!(prefix = %config.namespace_prefix, "waiting for the uploads to finish");
        failed_uploads = task.finish().await;
    }
    Ok(RecordedRoom {
        members: result?,
        failed_uploads,
    })
}

/// Returns once `stop` turns true, the room stops by itself (see
/// `AutoStop`) or the session closes, and every participant's recording has
/// ended. The files are grouped by participant, ordered by when each was
/// first recorded.
async fn follow_members(
    session: &Arc<Session>,
    namespace_prefix: &str,
    output_dir: &std::path::Path,
    limits: RecordingLimits,
    options: &Arc<BroadcastOptions>,
    mut stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<Vec<MemberFiles>> {
    session
        .subscriber()
        .subscribe_namespace(namespace_prefix.to_string())
        .await?;
    tracing::info!(prefix = namespace_prefix, "room recording started");
    let (member_stop_sender, member_stop) = watch::channel(false);
    let mut members: JoinSet<(String, Vec<RecordedFile>)> = JoinSet::new();
    let mut recorded_files = Vec::new();
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
                if let Ok((namespace, files)) = finished {
                    recorded.remove(&namespace);
                    recorded_files.push((namespace, files));
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
    recorded_files.extend(members.join_all().await);
    Ok(by_member(recorded_files))
}

/// A participant who joined again is recorded more than once; their files
/// make one participant.
fn by_member(recorded: Vec<(String, Vec<RecordedFile>)>) -> Vec<MemberFiles> {
    let mut members: Vec<MemberFiles> = Vec::new();
    for (namespace, files) in recorded {
        match members
            .iter_mut()
            .find(|member| member.namespace == namespace)
        {
            Some(member) => member.files.extend(files),
            None => members.push(MemberFiles { namespace, files }),
        }
    }
    for member in &mut members {
        member.files.sort_by_key(|file| file.capture_origin);
    }
    members.sort_by_key(|member| {
        member
            .files
            .iter()
            .filter_map(|file| file.capture_origin)
            .min()
    });
    members
}

/// Composites a finished room into `composite.mp4` in its directory. With an
/// upload store, the composite is uploaded next to the participants' files.
/// The upload task kept those files for it; they count against the queue's
/// budget until they are deleted afterwards.
struct CompositeJob {
    queue: CompositeQueue,
    output_dir: PathBuf,
    upload: Option<(Arc<dyn ObjectStore>, Path)>,
    cancel: watch::Receiver<bool>,
}

impl CompositeJob {
    async fn run(mut self, room: RecordedRoom, status: &Mutex<RoomStatus>) -> CompositeState {
        let sources: Vec<&PathBuf> = room
            .members
            .iter()
            .flat_map(|member| &member.files)
            .map(|file| &file.path)
            .collect();
        let mut reserved = 0;
        if self.upload.is_some() {
            for path in &sources {
                reserved += tokio::fs::metadata(path)
                    .await
                    .map_or(0, |metadata| metadata.len());
            }
        }
        let state = if self.queue.try_reserve(reserved) {
            let state = self.composite(&room.members, status).await;
            self.queue.release(reserved);
            state
        } else {
            CompositeState::Skipped("the files exceed the compositing budget".to_string())
        };
        if self.upload.is_some() {
            for path in sources {
                if room.failed_uploads.contains(path) {
                    continue;
                }
                if let Err(error) = tokio::fs::remove_file(path).await {
                    tracing::warn!(path = %path.display(), %error, "cannot delete an uploaded file");
                }
            }
        }
        state
    }

    async fn composite(
        &mut self,
        members: &[MemberFiles],
        status: &Mutex<RoomStatus>,
    ) -> CompositeState {
        let shut_down = || CompositeState::Skipped("the recorder shut down".to_string());
        let _turn = tokio::select! {
            turn = self.queue.turn() => turn,
            _ = self.cancel.wait_for(|cancel| *cancel) => return shut_down(),
        };
        lock(status).composite = Some(CompositeState::Composing);
        let output = self.output_dir.join(COMPOSITE_FILE_NAME);
        tracing::info!(path = %output.display(), "compositing the room");
        let result = tokio::select! {
            result = composite_room(members, &output) => result,
            _ = self.cancel.wait_for(|cancel| *cancel) => Err(anyhow!("the recorder shut down")),
        };
        let origin = match result {
            Ok(origin) => origin,
            Err(error) => {
                tracing::warn!(error = format!("{error:#}"), "compositing failed");
                let _ = tokio::fs::remove_file(&output).await;
                return CompositeState::Failed(format!("{error:#}"));
            }
        };
        if let Some((store, prefix)) = &self.upload {
            let task = UploadTask::run(Arc::clone(store), false);
            task.queue().upload(Upload {
                path: output,
                key: prefix.clone().join(COMPOSITE_FILE_NAME),
                capture_origin: Some(origin),
            });
            if !task.finish().await.is_empty() {
                return CompositeState::Failed(
                    "cannot upload the composite; it stays on disk".to_string(),
                );
            }
        }
        tracing::info!("room composited");
        CompositeState::Done
    }
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
    session: Arc<Session>,
    namespace: String,
    output_dir: &std::path::Path,
    options: Arc<BroadcastOptions>,
    stop: watch::Receiver<bool>,
    status: Arc<Mutex<RoomStatus>>,
) -> impl Future<Output = (String, Vec<RecordedFile>)> + use<> {
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
        let files = result.unwrap_or_else(|error| {
            tracing::warn!(%namespace, error = format!("{error:#}"), "participant recording failed");
            member.error = Some(format!("{error:#}"));
            Vec::new()
        });
        (namespace, files)
    }
}

fn lock(status: &Mutex<RoomStatus>) -> std::sync::MutexGuard<'_, RoomStatus> {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use mediapack::Timestamp;
    use object_store::{Attribute, ObjectStoreExt, memory::InMemory};

    use super::*;
    use crate::test_support::{scratch_dir, write_fixture_fmp4};

    const ORIGIN: Timestamp = Timestamp::from_micros(1_700_000_000_000_000);

    fn recorded(path: &std::path::Path, origin_ms: u64) -> RecordedFile {
        RecordedFile {
            path: path.to_path_buf(),
            capture_origin: Some(ORIGIN.saturating_add(Timestamp::from_millis(origin_ms))),
        }
    }

    fn room_status() -> Mutex<RoomStatus> {
        Mutex::new(RoomStatus {
            state: RoomState::Finished,
            members: Vec::new(),
            composite: Some(CompositeState::Waiting),
        })
    }

    /// The job and the cancel sender, which has to outlive it.
    fn composite_job(
        dir: &std::path::Path,
        budget_bytes: u64,
        store: Arc<InMemory>,
    ) -> (CompositeJob, watch::Sender<bool>) {
        let (cancel_sender, cancel) = watch::channel(false);
        let job = CompositeJob {
            queue: CompositeQueue::new(budget_bytes),
            output_dir: dir.to_path_buf(),
            upload: Some((store, Path::from("recordings/1"))),
            cancel,
        };
        (job, cancel_sender)
    }

    #[test]
    fn groups_a_rejoined_participant_and_orders_by_first_recording() {
        // Arrange: b recorded first; a left and joined again
        let recorded_files = vec![
            (
                "a".to_string(),
                vec![recorded(std::path::Path::new("a-2.mp4"), 5_000)],
            ),
            (
                "b".to_string(),
                vec![recorded(std::path::Path::new("b.mp4"), 0)],
            ),
            (
                "a".to_string(),
                vec![recorded(std::path::Path::new("a-1.mp4"), 1_000)],
            ),
        ];

        // Act
        let members = by_member(recorded_files);

        // Assert
        let order: Vec<(&str, Vec<&std::path::Path>)> = members
            .iter()
            .map(|member| {
                let paths = member
                    .files
                    .iter()
                    .map(|file| file.path.as_path())
                    .collect();
                (member.namespace.as_str(), paths)
            })
            .collect();
        assert_eq!(
            order,
            [
                ("b", vec![std::path::Path::new("b.mp4")]),
                (
                    "a",
                    vec![
                        std::path::Path::new("a-1.mp4"),
                        std::path::Path::new("a-2.mp4")
                    ]
                ),
            ]
        );
    }

    #[tokio::test]
    async fn uploads_the_composite_and_deletes_the_kept_files() {
        // Arrange
        let dir = scratch_dir();
        let source = dir.join("a.mp4");
        write_fixture_fmp4(&source);
        let store = Arc::new(InMemory::new());
        let room = RecordedRoom {
            members: vec![MemberFiles {
                namespace: "anon/room/a".to_string(),
                files: vec![recorded(&source, 0)],
            }],
            failed_uploads: Vec::new(),
        };

        // Act
        let (job, _cancel_sender) = composite_job(&dir, 1 << 30, Arc::clone(&store));
        let state = job.run(room, &room_status()).await;

        // Assert
        let object = store
            .get(&Path::from("recordings/1/composite.mp4"))
            .await
            .unwrap();
        let origin = object
            .attributes
            .get(&Attribute::Metadata("capture-origin-us".into()))
            .cloned();
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(state, CompositeState::Done);
        assert_eq!(origin, Some(ORIGIN.micros().to_string().into()));
        assert!(left.is_empty());
    }

    #[tokio::test]
    async fn skips_a_room_over_the_budget_and_keeps_only_the_files_not_uploaded() {
        // Arrange
        let dir = scratch_dir();
        let (uploaded, not_uploaded) = (dir.join("a.mp4"), dir.join("b.mp4"));
        write_fixture_fmp4(&uploaded);
        write_fixture_fmp4(&not_uploaded);
        let store = Arc::new(InMemory::new());
        let room = RecordedRoom {
            members: vec![MemberFiles {
                namespace: "anon/room/a".to_string(),
                files: vec![recorded(&uploaded, 0), recorded(&not_uploaded, 1_000)],
            }],
            failed_uploads: vec![not_uploaded.clone()],
        };

        // Act
        let (job, _cancel_sender) = composite_job(&dir, 1, Arc::clone(&store));
        let state = job.run(room, &room_status()).await;

        // Assert
        let (uploaded_left, not_uploaded_left) = (uploaded.exists(), not_uploaded.exists());
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(state, CompositeState::Skipped(_)));
        assert!(!uploaded_left);
        assert!(not_uploaded_left);
        assert!(
            store
                .get(&Path::from("recordings/1/composite.mp4"))
                .await
                .is_err()
        );
    }

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
