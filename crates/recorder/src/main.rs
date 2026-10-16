mod file_writer;
mod pipeline;
mod recorder;
mod recording;
mod relay;
#[cfg(test)]
mod test_support;
mod token_refresh_task;
mod track_selection;
mod upload_task;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

use crate::recording::Layout;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Record an MSF broadcast from a MoQT relay into fragmented MP4"
)]
pub(crate) struct Args {
    /// MoQT relay URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long, env = "MOQT_URL")]
    pub(crate) relay: String,

    /// Namespace of the broadcast, e.g. anon/live/test
    #[arg(long)]
    pub(crate) namespace: String,

    /// Name of the LOC video track listed in the catalog
    #[arg(long, default_value = "video")]
    pub(crate) video_track: String,

    /// Name of the LOC audio track listed in the catalog
    #[arg(long, default_value = "audio")]
    pub(crate) audio_track: String,

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
    /// audio-only file) after this many seconds; 60 by default with `--upload`
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
    recorder::record(Args::parse()).await
}
