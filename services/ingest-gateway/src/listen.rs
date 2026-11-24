mod ingest_publisher;
mod namespace;
mod renditions;
mod rtmp;
mod rtp;
mod srt;
mod stats_panel;

use std::path::PathBuf;

use anyhow::Result;

use self::{
    ingest_publisher::IngestOptions,
    stats_panel::{ConnectionRegistry, StatsPanel},
};
use crate::RelayArgs;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(flatten)]
    pub(crate) relay: RelayArgs,

    /// RTMP listen address (e.g. 0.0.0.0:1935)
    #[arg(long, default_value = "0.0.0.0:1935")]
    rtmp_addr: String,

    /// SRT listen address (e.g. 0.0.0.0:9000)
    #[arg(long, default_value = "0.0.0.0:9000")]
    srt_addr: String,

    /// SDP file describing the RTP session to receive; RTP input is off without it
    #[arg(long)]
    rtp_sdp: Option<PathBuf>,

    /// MoQT namespace the RTP session is published under
    #[arg(long, default_value = "anon/rtp/live")]
    rtp_namespace: String,

    /// Re-encode video into the standard renditions below the source resolution
    #[arg(long)]
    transcode: bool,

    /// Redraw the QUIC statistics of every relay connection on stdout once a second; logs move to stderr
    #[arg(long)]
    pub(crate) stats: bool,
}

pub(crate) async fn run(args: Args) -> Result<()> {
    let stats = args.stats.then(|| {
        let registry = ConnectionRegistry::default();
        (StatsPanel::run(registry.clone()), registry)
    });

    let options = IngestOptions {
        moqt: args.relay.moqt_target(),
        transcode: args.transcode,
        stats: stats.as_ref().map(|(_, registry)| registry.clone()),
    };
    let rtp_namespace = rtp::parse_namespace(&args.rtp_namespace)?;
    tokio::try_join!(
        rtmp::run_rtmp_listener(args.rtmp_addr, options.clone()),
        srt::run_srt_listener(args.srt_addr, options.clone()),
        async {
            match args.rtp_sdp {
                Some(sdp_path) => rtp::run_rtp_listener(sdp_path, rtp_namespace, options).await,
                None => Ok(()),
            }
        },
    )?;

    Ok(())
}
