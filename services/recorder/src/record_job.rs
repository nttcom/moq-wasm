use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::sync::watch;
use url::Url;

use crate::{
    RecordArgs, RoomArgs,
    bots_client::BotsClient,
    launcher::{Job, Launcher},
    recorder::{BroadcastOptions, Content},
    relay::ServiceToken,
    room_recording::{RecordingLimits, RoomConfig, RoomStatus, lock, record},
    room_state::{CompositeState, RoomRecord, RoomState},
    state_sync_task::{StateSyncTask, bot_state, now_ms, with_status},
    store::RecordingStore,
    token_refresh_task::TokenRenewal,
};

/// Records the room of recording `args.id` as the API wrote it into the
/// store. A job whose recording is no longer `starting` was given up by the
/// API and exits without recording.
pub(crate) async fn run(args: RecordArgs) -> Result<()> {
    let store = RecordingStore::open(&args.store)?;
    let record_state = store
        .read_state(&args.id)
        .await?
        .with_context(|| format!("no recording {}", args.id))?;
    ensure!(
        record_state.state == RoomState::Starting,
        "recording {} is {:?}, not starting",
        args.id,
        record_state.state
    );
    tracing::info!(id = %args.id, prefix = %record_state.namespace_prefix, "recording job started");
    record_room(RecordingJob {
        store,
        store_url: args.store,
        launcher: Launcher::from_args(&args.launcher)?,
        record_state,
        limits: args.limits.limits(),
        room: args.room,
        token: ServiceToken {
            token: args.token,
            renewal: args.temporary_apps_url.map(TokenRenewal::Issuer),
        },
        output_dir: args.output_dir,
        bots: None,
    })
    .await
}

/// A room recording a job runs, as `recorder serve` or `bots` asked for it.
pub(crate) struct RecordingJob {
    pub(crate) store: RecordingStore,
    pub(crate) store_url: Url,
    pub(crate) launcher: Launcher,
    /// Written to the store as `starting` before the job runs.
    pub(crate) record_state: RoomRecord,
    pub(crate) room: RoomArgs,
    pub(crate) limits: RecordingLimits,
    pub(crate) token: ServiceToken,
    pub(crate) output_dir: PathBuf,
    /// `bots`, when it started the recording. It holds the room, and the
    /// recording reports to it and publishes its presence. Without it the
    /// room's lock is `recorder serve`'s, released when the recording ends.
    pub(crate) bots: Option<Arc<BotsClient>>,
}

/// Records the room and keeps its state in the store while it runs. When it
/// ends it releases the room, or reports the end to `bots` when `bots`
/// started it, then starts the composite if one was requested.
pub(crate) async fn record_room(job: RecordingJob) -> Result<()> {
    let RecordingJob {
        store,
        store_url,
        launcher,
        record_state,
        room,
        limits,
        token,
        output_dir,
        bots,
    } = job;
    let id = record_state.id.clone();
    let prefix = record_state.namespace_prefix.clone();
    let status = Arc::new(Mutex::new(RoomStatus {
        state: RoomState::Recording,
        members: Vec::new(),
    }));
    let (stop_sender, stop) = watch::channel(false);
    let sync = StateSyncTask::run(
        store.clone(),
        record_state.clone(),
        Arc::clone(&status),
        stop_sender,
        bots.clone(),
    )?;
    let config = RoomConfig {
        relay: room.relay.clone(),
        verify_certificate: !room.insecure_skip_tls_verify,
        namespace_prefix: prefix.clone(),
        token,
        output_dir: output_dir.join(&id),
        store: store.object_store(),
        prefix: store.recording_prefix(&prefix, &id),
        options: BroadcastOptions {
            content: Content::Camera,
            video_track: None,
            max_video_height: room.max_video_height,
            audio_track: None,
            layout: record_state.layout,
            from_cache: false,
            segment_length: room.segment_seconds.map(Duration::from_secs),
            upload: None,
        },
        format: record_state.format,
        limits,
        presence: bots
            .as_ref()
            .map(|bots| format!("{prefix}/bots/{}", bots.bot_type())),
    };
    let result = record(config, stop, &status).await;
    sync.finish().await;
    let final_state = match &result {
        Ok(_) => RoomState::Finished,
        Err(error) => {
            tracing::error!(error = format!("{error:#}"), "room recording failed");
            RoomState::Failed(format!("{error:#}"))
        }
    };
    let finished = {
        let mut status = lock(&status);
        status.state = final_state;
        RoomRecord {
            updated_at_ms: now_ms(),
            ..with_status(&record_state, &status)
        }
    };
    store.write_state(&finished).await?;
    match &bots {
        Some(bots) => {
            let error = match &finished.state {
                RoomState::Failed(error) => Some(error.as_str()),
                _ => None,
            };
            if let Err(error) = bots.heartbeat(bot_state(&finished.state), error).await {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "cannot report the end to bots"
                );
            }
        }
        None => store.unlock_room(&prefix, &id).await?,
    }
    tracing::info!(%id, state = ?finished.state, "recording job finished");
    if record_state.composite {
        let composite = match &result {
            Ok(()) => launch_composite(&launcher, &id, &store_url, &output_dir).await,
            Err(_) => Some(CompositeState::Skipped("the recording failed".to_string())),
        };
        if let Some(composite) = composite {
            store.write_composite(&id, &composite).await?;
        }
    }
    result
}

/// `None` once the composite job runs: it reports its own state.
async fn launch_composite(
    launcher: &Launcher,
    id: &str,
    store_url: &Url,
    work_dir: &std::path::Path,
) -> Option<CompositeState> {
    let composite_args = vec![
        "--id".to_string(),
        id.to_string(),
        "--store".to_string(),
        store_url.to_string(),
        "--work-dir".to_string(),
        work_dir.display().to_string(),
    ];
    match launcher.launch(Job::Composite, composite_args).await {
        Ok(()) => None,
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "cannot launch the composite job"
            );
            Some(CompositeState::Failed(format!(
                "cannot launch the composite job: {error:#}"
            )))
        }
    }
}
