use std::sync::Arc;

use anyhow::{Context, Result};
use clap::ValueEnum;
use google_cloud::MetadataToken;
use job_launcher::JobLauncher;

use crate::BotArgs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LauncherKind {
    /// The same binary as a child process.
    Local,
    /// A Cloud Run Job execution, authenticated as the service account of
    /// the instance (the metadata server).
    CloudRun,
}

/// Starts `recorder composite` without waiting for it.
pub(crate) fn composite_launcher(args: &BotArgs) -> Result<JobLauncher> {
    Ok(match args.launcher {
        LauncherKind::Local => JobLauncher::Local {
            program: std::env::current_exe().context("cannot find the recorder binary")?,
        },
        LauncherKind::CloudRun => JobLauncher::CloudRun {
            job: args
                .composite_job
                .clone()
                .context("--launcher cloud-run needs --composite-job")?,
            client: reqwest::Client::new(),
            metadata_token: Arc::new(MetadataToken::new(reqwest::Client::new())),
        },
    })
}
