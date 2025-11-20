mod ingest;
mod moqt;
mod rtmp;
mod srt;

use anyhow::Result;
use clap::Parser;
use ffmpeg_next as ffmpeg;
use moqt::MoqtManager;

#[derive(Parser, Debug)]
#[command(author, version, about = "Listen RTMP/SRT and spawn handlers per stream")]
struct Args {
    /// RTMP listen address (e.g. 0.0.0.0:1935)
    #[arg(long, default_value = "0.0.0.0:1935")]
    rtmp_addr: String,

    /// SRT listen address (e.g. 0.0.0.0:9000)
    #[arg(long, default_value = "0.0.0.0:9000")]
    srt_addr: String,

    /// MoQ server URL (WebTransport)
    #[arg(long)]
    moqt_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // ffmpeg は後続のエンコード/転送処理を見据えて初期化だけ行う
    let _ = ffmpeg::init();

    let moqt = MoqtManager::new(args.moqt_url);

    let rtmp = tokio::spawn(rtmp::run_rtmp_listener(
        args.rtmp_addr,
        moqt.clone(),
    ));
    let srt = tokio::spawn(srt::run_srt_listener(args.srt_addr));

    rtmp.await??;
    srt.await??;

    Ok(())
}
