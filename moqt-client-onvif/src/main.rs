mod cli;
mod config;
mod http_client;
mod onvif_client;
mod onvif_command;
mod onvif_nodes;
mod onvif_profiles;
mod onvif_services;
mod ptz_config;
mod ptz_panel;
mod ptz_state;
mod ptz_worker;
mod rtsp_decode;
mod rtsp_types;
mod rtsp_worker;
mod soap;
mod video_view;
mod viewer;
mod wsse;

use anyhow::Result;
use clap::Parser;
use std::io::Write;

fn main() -> Result<()> {
    init_logger();
    let args = cli::Args::parse();
    let target = config::Target::from_args(&args)?;
    viewer::run(target)
}

fn init_logger() {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "info");
    env_logger::Builder::from_env(env)
        .format(|buf, record| {
            writeln!(
                buf,
                "{} {:<5} {}",
                buf.timestamp_millis(),
                record.level(),
                record.args()
            )
        })
        .init();
}
