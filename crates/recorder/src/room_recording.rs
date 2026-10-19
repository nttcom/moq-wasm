use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use moqt::{RequestErrorCode, Session, SessionEvent};
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path};
use tokio::{sync::watch, task::JoinSet, time::Instant};

use crate::{
    composite::MemberFiles,
    manifest::{MANIFEST_NAME, manifest_json},
    recorder::{BroadcastOptions, UploadTarget, default_output_path, record_broadcast},
    recording::RecordedFile,
    relay::RelayConnection,
    room_state::{MemberStatus, RoomState},
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
    /// The store the room's files are uploaded to, under `prefix`.
    pub(crate) store: Arc<dyn ObjectStore>,
    pub(crate) prefix: Path,
    pub(crate) options: BroadcastOptions,
    pub(crate) limits: RecordingLimits,
}

#[derive(Debug, Clone)]
pub(crate) struct RoomStatus {
    pub(crate) state: RoomState,
    pub(crate) members: Vec<MemberStatus>,
}

/// Records the room until `stop` turns true, the room stops by itself or the
/// session closes, then waits for the uploads and writes the manifest.
pub(crate) async fn record(
    config: RoomConfig,
    stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<()> {
    tokio::fs::create_dir_all(&config.output_dir)
        .await
        .with_context(|| format!("cannot create {}", config.output_dir.display()))?;
    let upload_task = UploadTask::run(Arc::clone(&config.store));
    let options = Arc::new(BroadcastOptions {
        upload: Some(UploadTarget {
            queue: upload_task.queue(),
            prefix: config.prefix.clone(),
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
    tracing::info!(prefix = %config.namespace_prefix, "waiting for the uploads to finish");
    let failed_uploads = upload_task.finish().await;
    let members = result?;
    let key = config.prefix.clone().join(MANIFEST_NAME);
    let manifest = manifest_json(&members, &failed_uploads).to_string();
    config
        .store
        .put(&key, PutPayload::from(manifest))
        .await
        .with_context(|| format!("cannot upload {key}"))?;
    Ok(())
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

pub(crate) fn stopping(status: &Mutex<RoomStatus>) {
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

pub(crate) fn lock(status: &Mutex<RoomStatus>) -> std::sync::MutexGuard<'_, RoomStatus> {
    status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use mediapack::Timestamp;

    use super::*;

    const ORIGIN: Timestamp = Timestamp::from_micros(1_700_000_000_000_000);

    fn recorded(path: &std::path::Path, origin_ms: u64) -> RecordedFile {
        RecordedFile {
            path: path.to_path_buf(),
            key: None,
            capture_origin: Some(ORIGIN.saturating_add(Timestamp::from_millis(origin_ms))),
        }
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
