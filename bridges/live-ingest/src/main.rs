mod chunk_payload;
mod ingest;
mod media_timeline;
mod moqt;
mod publisher;
mod renditions;
mod rtmp;
mod srt;

use anyhow::Result;
use clap::Parser;

use crate::publisher::IngestOptions;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Listen RTMP/SRT and spawn handlers per stream"
)]
struct Args {
    /// RTMP listen address (e.g. 0.0.0.0:1935)
    #[arg(long, default_value = "0.0.0.0:1935")]
    rtmp_addr: String,

    /// SRT listen address (e.g. 0.0.0.0:9000)
    #[arg(long, default_value = "0.0.0.0:9000")]
    srt_addr: String,

    /// MoQ server URL (`moqt://` for QUIC, `https://` for WebTransport)
    #[arg(long)]
    moqt_url: Option<String>,

    /// Re-encode video into the standard renditions below the source resolution
    #[arg(long)]
    transcode: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt::init();

    let options = IngestOptions {
        moqt_url: args.moqt_url,
        transcode: args.transcode,
    };
    tokio::try_join!(
        rtmp::run_rtmp_listener(args.rtmp_addr, options.clone()),
        srt::run_srt_listener(args.srt_addr, options),
    )?;

    Ok(())
}
