use std::sync::Arc;

use anyhow::{Context, Result};
use clap::ValueEnum;
use google_cloud::MetadataToken;
use job_launcher::JobLauncher;

use crate::LauncherArgs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LauncherKind {
    /// The same binary as a child process.
    Local,
    /// A Cloud Run Job execution, authenticated as the service account of
    /// the instance (the metadata server).
    CloudRun,
}

/// Starts `recorder composite` without waiting for it.
#[derive(Clone)]
pub(crate) struct Launcher {
    composite: JobLauncher,
}

impl Launcher {
    pub(crate) fn from_args(args: &LauncherArgs) -> Result<Self> {
        let composite = match args.launcher {
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
        };
        Ok(Self { composite })
    }

    /// `args` are the composite job's arguments after its mode.
    pub(crate) async fn launch_composite(&self, args: Vec<String>) -> Result<()> {
        let mut command = vec!["composite".to_string()];
        command.extend(args);
        self.composite.launch(command, Vec::new()).await?;
        tracing::info!("composite job launched");
        Ok(())
    }
}
