mod ingest_publisher;
mod namespace;
mod renditions;
mod rtmp;
mod rtp;
mod srt;
mod stats_panel;

use std::path::PathBuf;

use anyhow::Result;
use clap::ArgGroup;

use self::{
    ingest_publisher::IngestOptions,
    stats_panel::{ConnectionRegistry, StatsPanel},
};
use crate::RelayArgs;

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("input")
        .args(["rtmp_addr", "srt_addr", "rtp_sdp"])
        .required(true)
        .multiple(true)
))]
pub(crate) struct Args {
    #[command(flatten)]
    pub(crate) relay: RelayArgs,

    /// RTMP listen address (e.g. 0.0.0.0:1935); RTMP input is off without it
    #[arg(long)]
    rtmp_addr: Option<String>,

    /// SRT listen address (e.g. 0.0.0.0:9000); SRT input is off without it
    #[arg(long)]
    srt_addr: Option<String>,

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
    let (rtmp_options, srt_options) = (options.clone(), options.clone());
    tokio::try_join!(
        async {
            match args.rtmp_addr {
                Some(addr) => rtmp::run_rtmp_listener(addr, rtmp_options).await,
                None => Ok(()),
            }
        },
        async {
            match args.srt_addr {
                Some(addr) => srt::run_srt_listener(addr, srt_options).await,
                None => Ok(()),
            }
        },
        async {
            match args.rtp_sdp {
                Some(sdp_path) => rtp::run_rtp_listener(sdp_path, rtp_namespace, options).await,
                None => Ok(()),
            }
        },
    )?;

    Ok(())
}
