mod file_writer;
mod pipeline;
mod recorder;
mod relay;
mod track_selection;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

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

    /// Output file; defaults to <namespace>-<Unix seconds>.mp4 in the current directory
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,

    /// Authorization token (JWT) presented to the relay in CLIENT_SETUP
    #[arg(long, env = "MOQT_AUTH_TOKEN")]
    pub(crate) auth_token: Option<String>,

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
