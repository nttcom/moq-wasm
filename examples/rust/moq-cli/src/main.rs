mod cli;
mod publish;
mod subscribe;
mod telemetry;
mod track;
mod transport;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{
    EnvFilter, Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

use cli::{Cli, Command};
use client_telemetry::{ReportBuffer, report_layer};
use telemetry::{ReportTarget, ReportTask};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let report_target = cli
        .telemetry
        .endpoint(&cli.command.relay().url)
        .map(|endpoint| ReportTarget {
            endpoint,
            page: cli.command.name(),
            relay: cli.command.relay().clone(),
        });
    let buffer = ReportBuffer::default();
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()));
    let report_layer = report_target
        .is_some()
        .then(|| report_layer(buffer.clone()).with_filter(LevelFilter::INFO));
    tracing_subscriber::registry()
        .with(stderr_layer)
        .with(report_layer)
        .init();
    if report_target.is_some() {
        moqt::set_trace_context_provider(client_telemetry::current_traceparent);
    }
    let report_task = report_target.map(|target| ReportTask::run(target, buffer));

    let result = run(cli.command).await;
    if let Err(error) = &result {
        tracing::error!(error = format!("{error:#}"), "moq-cli failed");
    }
    if let Some(report_task) = report_task {
        report_task.finish().await;
    }
    if result.is_err() {
        // A pending blocking read on stdin would otherwise keep the runtime
        // from shutting down until stdin delivers data or EOF.
        std::process::exit(1);
    }
}

async fn run(command: Command) -> Result<()> {
    match command {
        Command::Publish(args) => publish::run(args).await,
        Command::Subscribe(args) => subscribe::run(args).await,
    }
}
