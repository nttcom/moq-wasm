mod api_server;
mod catalog_reader;
mod composite;
mod file_writer;
mod manifest;
mod pipeline;
mod record_job;
mod recorder;
mod recording;
mod recordings;
mod relay;
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

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::{recording::Layout, room_recording::RecordingLimits};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Record an MSF broadcast from a MoQT relay into fragmented MP4",
    after_help = "`recorder serve` serves an HTTP API that starts and stops room recordings; see `recorder serve --help`."
)]
pub(crate) struct Args {
    /// MoQT relay URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long, env = "MOQT_URL")]
    pub(crate) relay: String,

    /// Namespace of the broadcast, e.g. anon/live/test
    #[arg(long)]
    pub(crate) namespace: String,

    /// Name of the LOC video track listed in the catalog; without it the
    /// recorder picks one by `--max-video-height`
    #[arg(long)]
    pub(crate) video_track: Option<String>,

    /// Without `--video-track`, record the tallest video track at most this
    /// high, or the shortest one when all are taller
    #[arg(
        long,
        value_name = "PIXELS",
        default_value_t = 480,
        conflicts_with = "video_track"
    )]
    pub(crate) max_video_height: u32,

    /// Name of the LOC audio track listed in the catalog; defaults to the
    /// decodable audio track with the highest bitrate
    #[arg(long)]
    pub(crate) audio_track: Option<String>,

    /// Print the catalog's LOC tracks and exit instead of recording
    #[arg(long)]
    pub(crate) list_tracks: bool,

    /// Output file; defaults to <namespace>-<Unix seconds>.mp4 in the current
    /// directory. With `--layout per-track` each track goes to <stem>-<track>.mp4
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,

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

    /// First element of every namespace prefix a request may record; the API
    /// has no authentication, so it records anonymous rooms only by default
    #[arg(long, default_value = "anon")]
    pub(crate) namespace_root: String,

    /// Recordings that may run at once
    #[arg(long, default_value_t = 4)]
    pub(crate) max_recordings: usize,

    /// Directory the files are written to, one subdirectory per recording
    #[arg(long, default_value = "recordings")]
    pub(crate) output_dir: PathBuf,

    /// Upload every closed file under this object store location, then
    /// `<recording id>/<namespace>/`, and delete the local file
    #[arg(long, env = "RECORDER_UPLOAD_URL", value_name = "URL")]
    pub(crate) upload: Option<url::Url>,

    /// Accept `"composite": true`, which composites a room into one MP4 once
    /// its recording has ended
    #[arg(long)]
    pub(crate) composite: bool,

    /// With --composite and --upload, the most gigabytes of participants'
    /// files kept on disk for compositing at once; a room beyond it is not
    /// composited
    #[arg(long, value_name = "GB", default_value_t = 20)]
    pub(crate) composite_max_gb: u64,

    #[command(flatten)]
    pub(crate) room: RoomArgs,
}

/// Record one room that `recorder serve` was asked to record, then exit
#[derive(Parser, Debug)]
#[command(name = "recorder record", bin_name = "recorder record", version)]
pub(crate) struct RecordArgs {
    /// Id of the recording; the store holds its state and its room
    #[arg(long)]
    pub(crate) id: String,

    /// Object store holding the recordings and their state, e.g.
    /// gs://bucket/prefix or file:///directory
    #[arg(long, env = "RECORDER_STORE_URL", value_name = "URL")]
    pub(crate) store: url::Url,

    /// Directory the files are written to until they are uploaded
    #[arg(long, default_value = "recordings")]
    pub(crate) output_dir: PathBuf,

    #[command(flatten)]
    pub(crate) room: RoomArgs,
}

/// How a room is recorded: what `recorder serve` passes on to every
/// recording.
#[derive(clap::Args, Debug, Clone)]
pub(crate) struct RoomArgs {
    /// MoQT relay URL every recording connects to
    #[arg(long, env = "MOQT_URL")]
    pub(crate) relay: String,

    /// Start a new numbered file after this many seconds
    #[arg(long, value_name = "SECONDS")]
    pub(crate) segment_seconds: Option<u64>,

    /// Stop a recording after this many seconds
    #[arg(long, value_name = "SECONDS", default_value_t = 7200)]
    pub(crate) max_recording_seconds: u64,

    /// Stop a recording once no participant has been in the room for this
    /// many seconds, counted from the start too
    #[arg(long, value_name = "SECONDS", default_value_t = 300)]
    pub(crate) empty_room_seconds: u64,

    /// Record each participant's tallest video track at most this high, or
    /// the shortest one when all are taller
    #[arg(long, value_name = "PIXELS", default_value_t = 480)]
    pub(crate) max_video_height: u32,

    /// Skip TLS certificate verification (INSECURE, local development only)
    #[arg(long)]
    pub(crate) insecure_skip_tls_verify: bool,
}

impl RoomArgs {
    pub(crate) fn limits(&self) -> RecordingLimits {
        RecordingLimits {
            max_duration: Duration::from_secs(self.max_recording_seconds),
            empty_room_timeout: Duration::from_secs(self.empty_room_seconds),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with_writer(std::io::stderr)
        .init();
    // One parser per mode instead of clap subcommands: with subcommands, a
    // recording flag filled from an environment variable (`MOQT_URL`) counts
    // as given, so clap parses the recording mode, ignores the mode name and
    // asks for `--namespace`.
    let mode = std::env::args_os().nth(1);
    let mode_args = || std::env::args_os().skip(1);
    match mode.as_ref().and_then(|mode| mode.to_str()) {
        Some("serve") => serve::serve(ServeArgs::parse_from(mode_args())).await,
        Some("record") => record_job::run(RecordArgs::parse_from(mode_args())).await,
        _ => recorder::run(Args::parse()).await,
    }
}
