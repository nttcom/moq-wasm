use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, ensure};
use clap::ValueEnum;
use google_cloud::MetadataToken;
use job_launcher::JobLauncher;
use serde_json::Value;
use tokio::sync::watch;
use url::Url;

use crate::{
    BotArgs, RoomArgs,
    bot_dispatcher_client::{BotDispatcherClient, BotState},
    recorder::{BroadcastOptions, Content},
    recordings::recording_id,
    relay::ServiceToken,
    room_recording::{RecordingLimits, RoomConfig, RoomStatus, lock, record},
    room_state::{CompositeState, RecordingFormat, RecordingOptions, RoomRecord, RoomState},
    state_sync_task::{StateSyncTask, bot_state, now_ms, with_status},
    store::RecordingStore,
    token_refresh_task::TokenRenewal,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LauncherKind {
    /// The same binary as a child process.
    Local,
    /// A Cloud Run Job execution, authenticated as the service account of
    /// the instance (the metadata server).
    CloudRun,
}

/// Records the room `bot-dispatcher` started the bot for, under a new recording that
/// `recorder serve` lists like any other.
pub(crate) async fn run(args: BotArgs) -> Result<()> {
    let dispatcher = Arc::new(BotDispatcherClient::from_env(
        args.bot_dispatcher_url.clone(),
        args.bot_type.clone(),
        args.bot_id.clone(),
    ));
    let job = match prepare(args, &dispatcher).await {
        Ok(job) => job,
        Err(error) => {
            let message = format!("{error:#}");
            if let Err(report) = dispatcher.heartbeat(BotState::Failed, Some(&message)).await {
                tracing::warn!(
                    error = format!("{report:#}"),
                    "cannot report the failure to bot-dispatcher"
                );
            }
            return Err(error);
        }
    };
    job.record().await
}

/// A room recording `bot-dispatcher` started.
struct RecordingJob {
    store: RecordingStore,
    store_url: Url,
    launcher: JobLauncher,
    /// Written to the store as `starting` before the job runs.
    record_state: RoomRecord,
    room: RoomArgs,
    limits: RecordingLimits,
    token: ServiceToken,
    output_dir: PathBuf,
    /// Holds the room; the recording reports to it and publishes its
    /// presence.
    dispatcher: Arc<BotDispatcherClient>,
}

async fn prepare(args: BotArgs, dispatcher: &Arc<BotDispatcherClient>) -> Result<RecordingJob> {
    let options: Value = match &args.options {
        Some(options) => serde_json::from_str(options).context("--options is not JSON")?,
        None => Value::Object(Default::default()),
    };
    let options = RecordingOptions::from_json(&options).map_err(|error| anyhow!(error))?;
    ensure!(
        !(options.composite && options.format == RecordingFormat::Moq),
        "composite requires mp4 format"
    );
    let token = dispatcher
        .token()
        .await
        .context("cannot get a token from bot-dispatcher")?;
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
    dispatcher.set_recording_id(&record_state.id);
    store
        .add_to_room(&record_state.namespace_prefix, &record_state.id)
        .await?;
    tracing::info!(
        bot_id = %args.bot_id,
        id = %record_state.id,
        track_namespace_prefix = %record_state.namespace_prefix,
        "recording bot started"
    );
    let launcher = composite_launcher(&args)?;
    Ok(RecordingJob {
        store,
        store_url: args.store,
        launcher,
        record_state,
        room: args.room,
        limits: RecordingLimits {
            max_duration: Duration::from_secs(args.max_seconds),
            empty_room_timeout: Duration::from_secs(args.grace_seconds),
        },
        token: ServiceToken {
            renewal: token
                .is_some()
                .then(|| TokenRenewal::BotDispatcher(Arc::clone(dispatcher))),
            token,
        },
        output_dir: args.output_dir,
        dispatcher: Arc::clone(dispatcher),
    })
}

/// Starts `recorder composite` without waiting for it.
fn composite_launcher(args: &BotArgs) -> Result<JobLauncher> {
    Ok(match args.launcher {
        LauncherKind::Local => JobLauncher::Local {
            program: std::env::current_exe().context("cannot find the recorder binary")?,
        },
        LauncherKind::CloudRun => JobLauncher::CloudRun {
            job: args
                .composite_job
                .clone()
                .context("--launcher cloud-run needs --composite-job")?,
            client: reqwest::Client::new(),
            metadata_token: Arc::new(MetadataToken::new(reqwest::Client::new())),
        },
    })
}

impl RecordingJob {
    /// Records the room and keeps its state in the store while it runs, reports
    /// the end to `bot-dispatcher`, then starts the composite if one was requested.
    async fn record(self) -> Result<()> {
        let id = self.record_state.id.clone();
        let prefix = self.record_state.namespace_prefix.clone();
        let status = Arc::new(Mutex::new(RoomStatus {
            state: RoomState::Starting,
            members: Vec::new(),
        }));
        let (stop_sender, stop) = watch::channel(false);
        let sync = StateSyncTask::run(
            self.store.clone(),
            self.record_state.clone(),
            Arc::clone(&status),
            stop_sender,
            Arc::clone(&self.dispatcher),
        )?;
        let config = RoomConfig {
            relay: self.room.relay.clone(),
            verify_certificate: !self.room.insecure_skip_tls_verify,
            namespace_prefix: prefix.clone(),
            token: self.token,
            output_dir: self.output_dir.join(&id),
            store: self.store.object_store(),
            prefix: self.store.recording_prefix(&prefix, &id),
            options: BroadcastOptions {
                content: Content::Camera,
                video_track: None,
                max_video_height: self.room.max_video_height,
                audio_track: None,
                layout: self.record_state.layout,
                from_cache: false,
                segment_length: self.room.segment_seconds.map(Duration::from_secs),
                upload: None,
            },
            format: self.record_state.format,
            limits: self.limits,
            presence: format!("{prefix}/bots/{}", self.dispatcher.bot_type()),
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
                ..with_status(&self.record_state, &status)
            }
        };
        self.store.write_state(&finished).await?;
        let error = match &finished.state {
            RoomState::Failed(error) => Some(error.as_str()),
            _ => None,
        };
        if let Err(error) = self
            .dispatcher
            .heartbeat(bot_state(&finished.state), error)
            .await
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "cannot report the end to bot-dispatcher"
            );
        }
        tracing::info!(%id, state = ?finished.state, "recording job finished");
        if self.record_state.composite {
            let composite = match &result {
                Ok(()) => {
                    launch_composite(&self.launcher, &id, &self.store_url, &self.output_dir).await
                }
                Err(_) => Some(CompositeState::Skipped("the recording failed".to_string())),
            };
            if let Some(composite) = composite {
                self.store.write_composite(&id, &composite).await?;
            }
        }
        result
    }
}

/// `None` once the composite job runs: it reports its own state.
async fn launch_composite(
    launcher: &JobLauncher,
    id: &str,
    store_url: &Url,
    work_dir: &Path,
) -> Option<CompositeState> {
    let command = vec![
        "composite".to_string(),
        "--id".to_string(),
        id.to_string(),
        "--store".to_string(),
        store_url.to_string(),
        "--work-dir".to_string(),
        work_dir.display().to_string(),
    ];
    match launcher.launch(command, Vec::new()).await {
        Ok(()) => {
            tracing::info!(id, "composite job launched");
            None
        }
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
