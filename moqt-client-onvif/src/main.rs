use anyhow::Result;
use clap::Parser;
use moqt_client_onvif::{app_config, cli, ui_app};
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
