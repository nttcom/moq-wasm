mod chunk_payload;
mod ingest;
mod moqt;
mod publisher;
mod rtmp;
mod srt;

use anyhow::Result;
use clap::Parser;

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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let rtmp = tokio::spawn(rtmp::run_rtmp_listener(
        args.rtmp_addr,
        args.moqt_url.clone(),
    ));
    let srt = tokio::spawn(srt::run_srt_listener(args.srt_addr));

    rtmp.await??;
    srt.await??;

    Ok(())
}
