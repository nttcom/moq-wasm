mod app_config;
mod cli;
mod onvif_client;
mod onvif_nodes;
mod onvif_profiles;
mod onvif_requests;
mod onvif_services;
mod ptz_config;
mod ptz_state;
mod ptz_worker;
mod rtsp_decoder;
mod rtsp_frame;
mod rtsp_stream;
mod soap_client;
mod ui_app;
mod ui_ptz;
mod ui_video;
mod wsse_auth;

use anyhow::Result;
use clap::Parser;
use std::io::Write;

fn main() -> Result<()> {
    init_logger();
    let args = cli::Args::parse();
    let target = app_config::Target::from_args(&args)?;
    ui_app::run(target)
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
