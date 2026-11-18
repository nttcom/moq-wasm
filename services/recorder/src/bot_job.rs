use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::Value;

use crate::{
    BotArgs,
    bots_client::{BotState, BotsClient},
    launcher::Launcher,
    record_job::{RecordingJob, record_room},
    recordings::recording_id,
    relay::ServiceToken,
    room_recording::RecordingLimits,
    room_state::{RecordingFormat, RecordingOptions, RoomRecord, RoomState},
    state_sync_task::now_ms,
    store::RecordingStore,
    token_refresh_task::TokenRenewal,
};

/// Records the room `bots` started the bot for, under a new recording that
/// `recorder serve` lists like any other.
pub(crate) async fn run(args: BotArgs) -> Result<()> {
    let bots = Arc::new(BotsClient::from_env(
        args.bots_url.clone(),
        args.bot_type.clone(),
        args.bot_id.clone(),
    ));
    let job = match prepare(args, &bots).await {
        Ok(job) => job,
        Err(error) => {
            let message = format!("{error:#}");
            if let Err(report) = bots.heartbeat(BotState::Failed, Some(&message)).await {
                tracing::warn!(
                    error = format!("{report:#}"),
                    "cannot report the failure to bots"
                );
            }
            return Err(error);
        }
    };
    record_room(job).await
}

async fn prepare(args: BotArgs, bots: &Arc<BotsClient>) -> Result<RecordingJob> {
    let options: Value = match &args.options {
        Some(options) => serde_json::from_str(options).context("--options is not JSON")?,
        None => Value::Object(Default::default()),
    };
    let options = RecordingOptions::from_json(&options).map_err(|error| anyhow!(error))?;
    ensure!(
        !(options.composite && options.format == RecordingFormat::Moq),
        "composite requires mp4 format"
    );
    let token = bots.token().await.context("cannot get a token from bots")?;
    let store = RecordingStore::open(&args.store)?;
    let record_state = RoomRecord {
        id: recording_id(),
        namespace_prefix: args.namespace_prefix.clone(),
        layout: options.layout,
        format: options.format,
        composite: options.composite,
        state: RoomState::Starting,
        members: Vec::new(),
        updated_at_ms: now_ms(),
    };
    store.write_state(&record_state).await?;
    store
        .add_to_room(&record_state.namespace_prefix, &record_state.id)
        .await?;
    tracing::info!(
        bot_id = %args.bot_id,
        id = %record_state.id,
        track_namespace_prefix = %record_state.namespace_prefix,
        "recording bot started"
    );
    Ok(RecordingJob {
        store,
        store_url: args.store,
        launcher: Launcher::from_args(&args.launcher)?,
        record_state,
        room: args.room,
        limits: RecordingLimits {
            max_duration: Duration::from_secs(args.max_seconds),
            empty_room_timeout: Duration::from_secs(args.grace_seconds),
        },
        token: ServiceToken {
            renewal: token
                .is_some()
                .then(|| TokenRenewal::Bots(Arc::clone(bots))),
            token,
        },
        output_dir: args.output_dir,
        bots: Arc::clone(bots),
    })
}
