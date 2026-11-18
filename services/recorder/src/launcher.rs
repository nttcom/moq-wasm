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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Job {
    Record,
    Composite,
}

impl Job {
    pub(crate) fn mode(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Composite => "composite",
        }
    }
}

/// Starts `recorder record` and `recorder composite` without waiting for
/// them.
#[derive(Clone)]
pub(crate) struct Launcher {
    record: JobLauncher,
    composite: JobLauncher,
}

impl Launcher {
    pub(crate) fn from_args(args: &LauncherArgs) -> Result<Self> {
        Ok(match args.launcher {
            LauncherKind::Local => {
                let local = JobLauncher::Local {
                    program: std::env::current_exe().context("cannot find the recorder binary")?,
                };
                Self::both(local)
            }
            LauncherKind::CloudRun => {
                let client = reqwest::Client::new();
                let metadata_token = Arc::new(MetadataToken::new(client.clone()));
                let cloud_run = |job: &Option<String>, flag: &str| {
                    Ok::<_, anyhow::Error>(JobLauncher::CloudRun {
                        job: job
                            .clone()
                            .with_context(|| format!("--launcher cloud-run needs {flag}"))?,
                        client: client.clone(),
                        metadata_token: Arc::clone(&metadata_token),
                    })
                };
                Self {
                    record: cloud_run(&args.record_job, "--record-job")?,
                    composite: cloud_run(&args.composite_job, "--composite-job")?,
                }
            }
        })
    }

    pub(crate) fn both(launcher: JobLauncher) -> Self {
        Self {
            record: launcher.clone(),
            composite: launcher,
        }
    }

    /// `args` are the job's arguments after its mode.
    pub(crate) async fn launch(&self, job: Job, args: Vec<String>) -> Result<()> {
        let mut command = vec![job.mode().to_string()];
        command.extend(args);
        let launcher = match job {
            Job::Record => &self.record,
            Job::Composite => &self.composite,
        };
        launcher.launch(command).await?;
        tracing::info!(mode = job.mode(), "job launched");
        Ok(())
    }
}
