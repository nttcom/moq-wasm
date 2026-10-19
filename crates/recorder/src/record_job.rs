use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use tokio::sync::watch;

use crate::{
    RecordArgs,
    recorder::BroadcastOptions,
    room_recording::{RoomConfig, RoomStatus, lock, record},
    room_state::{RoomRecord, RoomState},
    state_sync_task::{StateSyncTask, now_ms, with_status},
    store::RecordingStore,
};

/// Records the room of recording `args.id` as the API wrote it into the
/// store, keeps its state there while it runs, and releases the room when it
/// ends. A job whose recording is no longer `starting` was given up by the
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
    let status = Arc::new(Mutex::new(RoomStatus {
        state: RoomState::Recording,
        members: Vec::new(),
        composite: None,
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
        upload: Some((store.object_store(), store.recording_prefix(&args.id))),
        options: BroadcastOptions {
            video_track: None,
            max_video_height: args.room.max_video_height,
            audio_track: None,
            layout: record_state.layout,
            from_cache: false,
            segment_length: args.room.segment_seconds.map(Duration::from_secs),
            upload: None,
        },
        limits: args.room.limits(),
        composite: None,
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
    result.map(drop)
}
