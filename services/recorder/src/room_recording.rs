use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use moqt::{RequestErrorCode, Session, SessionEvent};
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path};
use tokio::{
    sync::watch,
    task::{JoinError, JoinSet},
    time::Instant,
};

use crate::{
    archive::{record_broadcast as record_archive, record_event_track},
    archive_chunks::ChunkWriter,
    composite::MemberFiles,
    manifest::{MANIFEST_NAME, manifest_json},
    recorder::{BroadcastOptions, Content, UploadTarget, default_output_path, record_broadcast},
    recording::RecordedFile,
    relay::{RelayConnection, ServiceToken},
    room_state::{MemberStatus, RecordingFormat, RoomState},
    upload_task::UploadTask,
};

/// Where a room recording connects and what it records. Every broadcast whose
/// namespace starts with `namespace_prefix` is recorded on its own, from the
/// moment it is published until it ends or the recording stops.
pub(crate) struct RoomConfig {
    pub(crate) relay: String,
    pub(crate) verify_certificate: bool,
    pub(crate) namespace_prefix: String,
    pub(crate) token: ServiceToken,
    pub(crate) output_dir: PathBuf,
    /// The store the room's files are uploaded to, under `prefix`.
    pub(crate) store: Arc<dyn ObjectStore>,
    pub(crate) prefix: Path,
    pub(crate) options: BroadcastOptions,
    pub(crate) format: RecordingFormat,
    pub(crate) limits: RecordingLimits,
}

#[derive(Debug, Clone)]
pub(crate) struct RoomStatus {
    pub(crate) state: RoomState,
    pub(crate) members: Vec<MemberStatus>,
}

struct ArchiveTarget {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
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
        ..config.options.clone()
    });
    let archive_target = (config.format == RecordingFormat::Moq).then(|| {
        Arc::new(ArchiveTarget {
            store: Arc::clone(&config.store),
            prefix: config.prefix.clone().join("archive"),
        })
    });
    let connection = RelayConnection::connect_service(
        &config.relay,
        config.verify_certificate,
        &config.namespace_prefix,
        &config.token,
    )
    .await?;
    let result = follow_members(
        &connection.session,
        &config,
        &options,
        archive_target,
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
    if config.format == RecordingFormat::Moq {
        let errors: Vec<String> = lock(status)
            .members
            .iter()
            .filter_map(|member| {
                member
                    .error
                    .as_ref()
                    .map(|error| format!("{}: {error}", member.namespace))
            })
            .collect();
        if !errors.is_empty() {
            anyhow::bail!("archive recording failed: {}", errors.join("; "));
        }
    }
    let key = config.prefix.clone().join(MANIFEST_NAME);
    let manifest = match config.format {
        RecordingFormat::Mp4 => manifest_json(&members, &failed_uploads),
        RecordingFormat::Moq => serde_json::json!({
            "version": 1,
            "format": "moq",
            "members": members.iter().map(|member| serde_json::json!({
                "namespace": member.namespace,
                "camera": member.files.iter().filter_map(|file| file.key.as_ref().map(|key| key.as_ref())).collect::<Vec<_>>(),
                "screen_share": member.screen_share.iter().filter_map(|file| file.key.as_ref().map(|key| key.as_ref())).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }),
    }.to_string();
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
    config: &RoomConfig,
    options: &Arc<BroadcastOptions>,
    archive_target: Option<Arc<ArchiveTarget>>,
    mut stop: watch::Receiver<bool>,
    status: &Arc<Mutex<RoomStatus>>,
) -> Result<Vec<MemberFiles>> {
    let namespace_prefix = config.namespace_prefix.as_str();
    let output_dir = config.output_dir.as_path();
    session
        .subscriber()
        .subscribe_namespace(namespace_prefix.to_string())
        .await?;
    tracing::info!(prefix = namespace_prefix, "room recording started");
    let (member_stop_sender, member_stop) = watch::channel(false);
    let mut members: JoinSet<MemberFiles> = JoinSet::new();
    let mut recorded_files = Vec::new();
    let mut recorded: HashSet<String> = HashSet::new();
    let mut event_sources: HashSet<String> = HashSet::new();
    let mut auto_stop = AutoStop::new(Instant::now(), config.limits);
    let mut failure = None;
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
                if archive_target.is_some() {
                    failure = Some(anyhow::anyhow!("relay session closed: {close}"));
                }
                break;
            }
            Some(finished) = members.join_next(), if !members.is_empty() => {
                match member_finished(finished, archive_target.is_some()) {
                    Ok(Some(files)) => {
                        recorded.remove(&files.namespace);
                        event_sources.remove(&files.namespace);
                        recorded_files.push(files);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
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
                if is_recording_namespace(namespace_prefix, &namespace) {
                    continue;
                }
                if namespace == format!("{namespace_prefix}/bots/transcriber")
                    && let Some(target) = &archive_target
                {
                    if event_sources.insert(namespace.clone()) {
                        members.spawn(record_transcript(
                            Arc::clone(session),
                            namespace,
                            output_dir,
                            Arc::clone(target),
                            member_stop.clone(),
                            Arc::clone(status),
                        ));
                    }
                    continue;
                }
                if !recorded.insert(namespace.clone()) {
                    continue;
                }
                tracing::info!(%namespace, "recording a participant");
                members.spawn(record_member(
                    Arc::clone(session),
                    namespace,
                    output_dir,
                    Arc::clone(options),
                    archive_target.clone(),
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
            SessionEvent::Disconnected() => {
                if archive_target.is_some() {
                    failure = Some(anyhow::anyhow!("relay session disconnected"));
                }
                break;
            }
            event => tracing::debug!(?event, "room session event ignored"),
        }
    }
    let _ = member_stop_sender.send(true);
    while let Some(result) = members.join_next().await {
        match member_finished(result, archive_target.is_some()) {
            Ok(files) => recorded_files.extend(files),
            Err(error) => failure = Some(error),
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(by_member(recorded_files))
}

/// A failed participant task fails a MoQ room recording, whose archive would
/// miss objects; an MP4 recording leaves that participant out.
fn member_finished(
    result: Result<MemberFiles, JoinError>,
    archive: bool,
) -> Result<Option<MemberFiles>> {
    match result {
        Ok(files) => Ok(Some(files)),
        Err(error) if archive => Err(error.into()),
        Err(error) => {
            tracing::error!(%error, "participant recording task failed");
            Ok(None)
        }
    }
}

fn is_recording_namespace(prefix: &str, namespace: &str) -> bool {
    namespace
        .strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix("/recordings/"))
        .is_some_and(|suffix| !suffix.is_empty())
}

/// A participant who joined again is recorded more than once; their files
/// make one participant.
fn by_member(recorded: Vec<MemberFiles>) -> Vec<MemberFiles> {
    let mut members: Vec<MemberFiles> = Vec::new();
    for recording in recorded {
        match members
            .iter_mut()
            .find(|member| member.namespace == recording.namespace)
        {
            Some(member) => {
                member.files.extend(recording.files);
                member.screen_share.extend(recording.screen_share);
            }
            None => members.push(recording),
        }
    }
    for member in &mut members {
        member.files.sort_by_key(|file| file.capture_origin);
        member.screen_share.sort_by_key(|file| file.capture_origin);
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

/// Records the camera and the audio, and apart from them the screen share,
/// each from its own catalog subscription.
fn record_member(
    session: Arc<Session>,
    namespace: String,
    output_dir: &std::path::Path,
    options: Arc<BroadcastOptions>,
    archive_target: Option<Arc<ArchiveTarget>>,
    stop: watch::Receiver<bool>,
    status: Arc<Mutex<RoomStatus>>,
) -> impl Future<Output = MemberFiles> + use<> {
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
    let share_path = output_dir.join(default_output_path(
        &format!("{namespace}/screenshare"),
        now,
    ));
    let share_options = BroadcastOptions {
        content: Content::ScreenShare,
        ..BroadcastOptions::clone(&options)
    };
    async move {
        let is_archive = archive_target.is_some();
        let (camera, screen_share) = if let Some(target) = archive_target {
            let mut prefix = target.prefix.clone();
            prefix.extend(namespace.split('/'));
            prefix = prefix.join(format!(
                "{}-{index}",
                now.duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
            ));
            let camera_chunks = ChunkWriter::new(
                Arc::clone(&target.store),
                prefix.clone().join("camera"),
                namespace.clone(),
            );
            let share_chunks = ChunkWriter::new(
                Arc::clone(&target.store),
                prefix.join("screenshare"),
                namespace.clone(),
            );
            let (camera, screen_share) = tokio::join!(
                archive_member(
                    &session,
                    &namespace,
                    path,
                    &options,
                    camera_chunks,
                    stop.clone()
                ),
                archive_member(
                    &session,
                    &namespace,
                    share_path,
                    &share_options,
                    share_chunks,
                    stop
                ),
            );
            (camera, screen_share)
        } else {
            tokio::join!(
                record_broadcast(&session, &namespace, path, &options, stop.clone()),
                record_broadcast(&session, &namespace, share_path, &share_options, stop),
            )
        };
        let mut status = lock(&status);
        let member = &mut status.members[index];
        member.finished = true;
        let files = camera.unwrap_or_else(|error| {
            tracing::warn!(%namespace, error = format!("{error:#}"), "participant recording failed");
            member.error = Some(format!("{error:#}"));
            Vec::new()
        });
        let screen_share = screen_share.unwrap_or_else(|error| {
            tracing::warn!(%namespace, error = format!("{error:#}"), "screen share recording failed");
            if is_archive && member.error.is_none() { member.error = Some(format!("screen share: {error:#}")); }
            Vec::new()
        });
        MemberFiles {
            namespace,
            files,
            screen_share,
        }
    }
}

async fn archive_member(
    session: &Session,
    namespace: &str,
    path: PathBuf,
    options: &BroadcastOptions,
    chunks: ChunkWriter,
    mut stop: watch::Receiver<bool>,
) -> Result<Vec<RecordedFile>> {
    let key = record_archive(
        session,
        namespace,
        None,
        options,
        Some(chunks),
        async move {
            let _ = stop.wait_for(|stop| *stop).await;
        },
    )
    .await?;
    Ok(key
        .into_iter()
        .map(|key| RecordedFile {
            path: path.clone(),
            key: Some(key),
            capture_origin: None,
        })
        .collect())
}

fn record_transcript(
    session: Arc<Session>,
    namespace: String,
    output_dir: &std::path::Path,
    target: Arc<ArchiveTarget>,
    mut stop: watch::Receiver<bool>,
    status: Arc<Mutex<RoomStatus>>,
) -> impl Future<Output = MemberFiles> + use<> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let index = {
        let mut status = lock(&status);
        status.members.push(MemberStatus {
            namespace: namespace.clone(),
            started_at_ms: now,
            finished: false,
            error: None,
        });
        status.members.len() - 1
    };
    let path = output_dir.join(format!("transcript-{now}-{index}.jsonl"));
    let mut prefix = target.prefix.clone();
    prefix.extend(namespace.split('/'));
    let chunks = ChunkWriter::new(
        Arc::clone(&target.store),
        prefix.join(format!("{now}-{index}")),
        namespace.clone(),
    );
    async move {
        let result = record_event_track(
            &session,
            &namespace,
            "transcript",
            None,
            chunks,
            async move {
                let _ = stop.wait_for(|stop| *stop).await;
            },
        )
        .await;
        let mut member = lock(&status);
        member.members[index].finished = true;
        let files = match result {
            Ok(key) => key
                .into_iter()
                .map(|key| RecordedFile {
                    path: path.clone(),
                    key: Some(key),
                    capture_origin: None,
                })
                .collect(),
            Err(error) => {
                member.members[index].error = Some(format!("transcript: {error:#}"));
                Vec::new()
            }
        };
        MemberFiles {
            namespace,
            files,
            screen_share: Vec::new(),
        }
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

    #[test]
    fn excludes_replayed_recordings_but_not_a_participant_named_recordings() {
        // Arrange
        let prefix = "app/room";

        // Act / Assert
        assert!(is_recording_namespace(
            prefix,
            "app/room/recordings/id/alice"
        ));
        assert!(!is_recording_namespace(prefix, "app/room/recordings"));
        assert!(!is_recording_namespace(
            prefix,
            "app/room/recordings-old/id"
        ));
        assert!(!is_recording_namespace(prefix, "app/room2/recordings/id"));
        assert!(!is_recording_namespace(prefix, "app/room/alice"));
    }

    fn recorded(path: &std::path::Path, origin_ms: u64) -> RecordedFile {
        RecordedFile {
            path: path.to_path_buf(),
            key: None,
            capture_origin: Some(ORIGIN.saturating_add(Timestamp::from_millis(origin_ms))),
        }
    }

    fn member(namespace: &str, files: &[(&str, u64)], screen_share: &[(&str, u64)]) -> MemberFiles {
        let recorded_files = |files: &[(&str, u64)]| {
            files
                .iter()
                .map(|(path, origin_ms)| recorded(std::path::Path::new(path), *origin_ms))
                .collect()
        };
        MemberFiles {
            namespace: namespace.to_string(),
            files: recorded_files(files),
            screen_share: recorded_files(screen_share),
        }
    }

    async fn panicked_member() -> Result<MemberFiles, JoinError> {
        tokio::spawn(async { panic!("participant task panicked") }).await
    }

    #[tokio::test]
    async fn a_panicked_participant_is_left_out_of_an_mp4_recording() {
        // Arrange
        let result = panicked_member().await;

        // Act
        let files = member_finished(result, false);

        // Assert
        assert!(files.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_panicked_participant_fails_a_moq_recording() {
        // Arrange
        let result = panicked_member().await;

        // Act
        let files = member_finished(result, true);

        // Assert
        assert!(files.is_err());
    }

    #[test]
    fn groups_a_rejoined_participant_and_orders_by_first_recording() {
        // Arrange: b recorded first; a left and joined again, sharing the screen both times
        let recorded_files = vec![
            member("a", &[("a-2.mp4", 5_000)], &[("a-share-2.mp4", 6_000)]),
            member("b", &[("b.mp4", 0)], &[]),
            member("a", &[("a-1.mp4", 1_000)], &[("a-share-1.mp4", 2_000)]),
        ];

        // Act
        let members = by_member(recorded_files);

        // Assert
        assert_eq!(
            members,
            [
                member("b", &[("b.mp4", 0)], &[]),
                member(
                    "a",
                    &[("a-1.mp4", 1_000), ("a-2.mp4", 5_000)],
                    &[("a-share-1.mp4", 2_000), ("a-share-2.mp4", 6_000)]
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
