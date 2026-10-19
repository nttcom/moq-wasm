use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::sync::watch;

use crate::{
    RecordArgs,
    launcher::{Job, Launcher},
    recorder::{BroadcastOptions, Content},
    room_recording::{RoomConfig, RoomStatus, lock, record},
    room_state::{CompositeState, RoomRecord, RoomState},
    state_sync_task::{StateSyncTask, now_ms, with_status},
    store::RecordingStore,
};

/// Records the room of recording `args.id` as the API wrote it into the
/// store, keeps its state there while it runs, and releases the room when it
/// ends; then starts the composite if one was requested. A job whose
/// recording is no longer `starting` was given up by the API and exits
/// without recording.
pub(crate) async fn run(args: RecordArgs) -> Result<()> {
    let store = RecordingStore::open(&args.store)?;
    let launcher = Launcher::from_args(&args.launcher)?;
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
    )?;
    let config = RoomConfig {
        relay: args.room.relay.clone(),
        verify_certificate: !args.room.insecure_skip_tls_verify,
        namespace_prefix: record_state.namespace_prefix.clone(),
        output_dir: args.output_dir.join(&args.id),
        store: store.object_store(),
        prefix: store.recording_prefix(&args.id),
        options: BroadcastOptions {
            content: Content::Camera,
            video_track: None,
            max_video_height: args.room.max_video_height,
            audio_track: None,
            layout: record_state.layout,
            from_cache: false,
            segment_length: args.room.segment_seconds.map(Duration::from_secs),
            upload: None,
        },
        limits: args.room.limits(),
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
    store
        .unlock_room(&record_state.namespace_prefix, &args.id)
        .await?;
    tracing::info!(id = %args.id, state = ?finished.state, "recording job finished");
    if record_state.composite {
        let composite = match &result {
            Ok(()) => launch_composite(&launcher, &args).await,
            Err(_) => Some(CompositeState::Skipped("the recording failed".to_string())),
        };
        if let Some(composite) = composite {
            store.write_composite(&args.id, &composite).await?;
        }
    }
    result
}

/// `None` once the composite job runs: it reports its own state.
async fn launch_composite(launcher: &Launcher, args: &RecordArgs) -> Option<CompositeState> {
    let composite_args = vec![
        "--id".to_string(),
        args.id.clone(),
        "--store".to_string(),
        args.store.to_string(),
        "--work-dir".to_string(),
        args.output_dir.display().to_string(),
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
