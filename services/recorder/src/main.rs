mod api_auth;
mod api_server;
mod archive;
mod archive_chunks;
mod archive_reader;
mod archive_receiver;
mod bot_job;
mod bots_client;
mod catalog_reader;
mod composite;
mod composite_job;
mod file_writer;
mod launcher;
mod manifest;
mod pipeline;
mod record_job;
mod recorder;
mod recording;
mod recording_clock;
mod recordings;
mod relay;
mod replay;
mod replay_stream;
mod room_recording;
mod room_state;
mod serve;
mod state_sync_task;
mod store;
#[cfg(test)]
mod test_support;
mod token_refresh_task;
mod track_list;
mod track_selection;
mod upload_task;
mod vod;

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Result;
use clap::Parser;

use crate::{launcher::LauncherKind, recording::Layout};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Record an MSF broadcast from a MoQT relay into fragmented MP4",
    after_help = "`recorder serve` serves an HTTP API that lists and replays room recordings; see `recorder serve --help`."
)]
pub(crate) struct Args {
    /// MoQT relay URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long, env = "MOQT_URL")]
    pub(crate) relay: String,

    /// Namespace of the broadcast, e.g. anon/live/test
    #[arg(long)]
    pub(crate) namespace: String,

    /// Name of the LOC video track listed in the catalog; without it the
    /// recorder picks the main source's by `--max-video-height`
    #[arg(long)]
    pub(crate) video_track: Option<String>,

    /// Without `--video-track`, record the main source's tallest video track
    /// at most this high, or the shortest one when all are taller
    #[arg(
        long,
        value_name = "PIXELS",
        default_value_t = 720,
        conflicts_with = "video_track"
    )]
    pub(crate) max_video_height: u32,

    /// Name of the LOC audio track listed in the catalog; defaults to the
    /// main source's decodable audio track with the highest bitrate
    #[arg(long)]
    pub(crate) audio_track: Option<String>,

    /// Print the catalog's LOC tracks and exit instead of recording
    #[arg(long)]
    pub(crate) list_tracks: bool,

    /// Output file; defaults to <namespace>-<Unix seconds>.mp4 in the current
    /// directory. With `--layout per-track` each track goes to <stem>-<track>.mp4
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,

    /// Record LOC objects and catalog updates as a JSON Lines archive
    #[arg(long, conflicts_with_all = ["segment_seconds", "from_cache", "list_tracks"])]
    pub(crate) archive: bool,

    /// Whether the video and the audio share one file or each track gets its own
    #[arg(long, value_enum, default_value_t = Layout::Combined)]
    pub(crate) layout: Layout,

    /// Start with what the relay or the publisher still caches, then continue live
    #[arg(long)]
    pub(crate) from_cache: bool,

    /// Start a new numbered file at the first keyframe (audio frame in an
    /// audio-only file) after this many seconds
    #[arg(long, value_name = "SECONDS")]
    pub(crate) segment_seconds: Option<u64>,

    /// Upload every closed file to this object store location, e.g.
    /// gs://bucket/prefix, and delete the local file once it is uploaded.
    /// Credentials come from the environment
    #[arg(long, env = "RECORDER_UPLOAD_URL", value_name = "URL")]
    pub(crate) upload: Option<url::Url>,

    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN", conflicts_with = "auth_token_file")]
    pub(crate) auth_token: Option<String>,

    /// File holding the authorization token; re-read while recording so a
    /// rewritten token is sent to the relay before the current one expires
    #[arg(long, env = "MOQT_AUTH_TOKEN_FILE")]
    pub(crate) auth_token_file: Option<PathBuf>,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    pub(crate) insecure_skip_tls_verify: bool,
}

/// Serve an HTTP API that starts and stops room recordings
#[derive(Parser, Debug)]
#[command(name = "recorder serve", bin_name = "recorder serve", version)]
pub(crate) struct ServeArgs {
    /// Address the HTTP API listens on
    #[arg(long, default_value = "127.0.0.1:8096")]
    pub(crate) listen: SocketAddr,

    /// Origin of a page allowed to call the API (CORS); repeat for several
    #[arg(
        long = "allowed-origin",
        value_name = "ORIGIN",
        default_values_t = ["http://localhost:5173".to_string(), "http://127.0.0.1:5173".to_string()]
    )]
    pub(crate) allowed_origins: Vec<String>,

    /// VTS verification endpoint for the bearer token every request carries
    #[arg(
        long,
        env = "AUTH_VTS_URL",
        required_unless_present = "insecure_skip_auth"
    )]
    pub(crate) auth_vts_url: Option<String>,

    /// Accept every request without verifying its token, and replay with the
    /// token it carries or none (INSECURE, local development only)
    #[arg(long, conflicts_with = "auth_vts_url")]
    pub(crate) insecure_skip_auth: bool,

    /// Recordings that may be replayed at once
    #[arg(long, default_value_t = 4)]
    pub(crate) max_replays: usize,

    /// Object store holding the recordings and their state, e.g.
    /// gs://bucket/prefix or file:///directory
    #[arg(long, env = "RECORDER_STORE_URL", value_name = "URL")]
    pub(crate) store: url::Url,

    /// Temporary-apps service that renews temporary app tokens: a replay of
    /// a temporary app's room (`temp-…` root) renews the caller's token there
    /// instead of ending when it expires
    #[arg(long, env = "TEMPORARY_APPS_URL")]
    pub(crate) temporary_apps_url: Option<String>,

    /// MoQT relay URL a replay publishes through
    #[arg(long, env = "MOQT_URL")]
    pub(crate) relay: String,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    pub(crate) insecure_skip_tls_verify: bool,
}

/// Record a room as the bot `bots` started, then exit
/// (docs/architecture/bots/architecture.md)
#[derive(Parser, Debug)]
#[command(name = "recorder bot", bin_name = "recorder bot", version)]
pub(crate) struct BotArgs {
    /// Type `bots` started the bot as, the name in its types file
    #[arg(long)]
    pub(crate) bot_type: String,

    /// Id `bots` gave the bot
    #[arg(long)]
    pub(crate) bot_id: String,

    /// URL of `bots`, which signs the bot's token and takes its heartbeats
    #[arg(long)]
    pub(crate) bots_url: String,

    /// The room: every broadcast under this prefix is recorded
    #[arg(long)]
    pub(crate) namespace_prefix: String,

    /// Stop once no participant has been in the room for this many seconds,
    /// counted from the start too
    #[arg(long, value_name = "SECONDS")]
    pub(crate) grace_seconds: u64,

    /// Stop after this many seconds
    #[arg(long, value_name = "SECONDS")]
    pub(crate) max_seconds: u64,

    /// `layout`, `composite` and `format` of the recording, as a JSON object
    #[arg(long, value_name = "JSON")]
    pub(crate) options: Option<String>,

    /// Object store holding the recordings and their state, e.g.
    /// gs://bucket/prefix or file:///directory
    #[arg(long, env = "RECORDER_STORE_URL", value_name = "URL")]
    pub(crate) store: url::Url,

    /// Directory the files are written to until they are uploaded
    #[arg(long, default_value = "recordings")]
    pub(crate) output_dir: PathBuf,

    #[command(flatten)]
    pub(crate) room: RoomArgs,

    #[command(flatten)]
    pub(crate) launcher: LauncherArgs,
}

/// Composite one room recording into one MP4, then exit
#[derive(Parser, Debug)]
#[command(name = "recorder composite", bin_name = "recorder composite", version)]
pub(crate) struct CompositeArgs {
    /// Id of the recording; the store holds its manifest
    #[arg(long)]
    pub(crate) id: String,

    /// Object store holding the recordings and their state, e.g.
    /// gs://bucket/prefix or file:///directory
    #[arg(long, env = "RECORDER_STORE_URL", value_name = "URL")]
    pub(crate) store: url::Url,

    /// Directory the recording's files are downloaded to while compositing
    #[arg(long, default_value = "composites")]
    pub(crate) work_dir: PathBuf,
}

/// How a room is recorded: the settings of a `recorder bot` job.
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct RoomArgs {
    /// MoQT relay URL the recording connects to
    #[arg(long, env = "MOQT_URL")]
    pub(crate) relay: String,

    /// Start a new numbered file after this many seconds
    #[arg(long, value_name = "SECONDS")]
    pub(crate) segment_seconds: Option<u64>,

    /// Record the tallest video track of each recorded source at most this
    /// high, or the shortest one when all are taller
    #[arg(long, value_name = "PIXELS", default_value_t = 720)]
    pub(crate) max_video_height: u32,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    pub(crate) insecure_skip_tls_verify: bool,
}

/// Where the composite job runs.
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct LauncherArgs {
    /// Run the composite job as a child process of this binary or as a Cloud Run Job
    #[arg(long, value_enum, default_value_t = LauncherKind::Local)]
    pub(crate) launcher: LauncherKind,

    /// Cloud Run Job running `recorder composite`, as
    /// projects/<project>/locations/<region>/jobs/<job>
    #[arg(long, env = "RECORDER_COMPOSITE_JOB")]
    pub(crate) composite_job: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _telemetry = telemetry_otlp::init(telemetry_otlp::TelemetryConfig {
        service_name: "recorder",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &[
            "recorder=info",
            "moqt=info",
            "msf=info",
            "mediapack=info",
            "transcode=info",
        ],
        log_output: telemetry_otlp::LogOutput::Stderr,
        ..Default::default()
    })?;
    // One parser per mode instead of clap subcommands: with subcommands, a
    // recording flag filled from an environment variable (`MOQT_URL`) counts
    // as given, so clap parses the recording mode, ignores the mode name and
    // asks for `--namespace`.
    let mode = std::env::args_os().nth(1);
    let mode_args = || std::env::args_os().skip(1);
    match mode.as_ref().and_then(|mode| mode.to_str()) {
        Some("serve") => serve::serve(ServeArgs::parse_from(mode_args())).await,
        Some("bot") => bot_job::run(BotArgs::parse_from(mode_args())).await,
        Some("composite") => composite_job::run(CompositeArgs::parse_from(mode_args())).await,
        _ => recorder::run(Args::parse()).await,
    }
}
