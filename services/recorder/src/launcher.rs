use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use serde_json::{Value, json};

use crate::LauncherArgs;

const METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

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
/// them: as a child process locally, or as a Cloud Run Job execution whose
/// container arguments are replaced by the given ones.
#[derive(Clone)]
pub(crate) enum Launcher {
    Local {
        program: PathBuf,
    },
    CloudRun {
        /// `projects/<project>/locations/<region>/jobs/<job>`
        record_job: String,
        composite_job: String,
        client: reqwest::Client,
    },
}

impl Launcher {
    pub(crate) fn from_args(args: &LauncherArgs) -> Result<Self> {
        Ok(match args.launcher {
            LauncherKind::Local => Self::Local {
                program: std::env::current_exe().context("cannot find the recorder binary")?,
            },
            LauncherKind::CloudRun => Self::CloudRun {
                record_job: args
                    .record_job
                    .clone()
                    .context("--launcher cloud-run needs --record-job")?,
                composite_job: args
                    .composite_job
                    .clone()
                    .context("--launcher cloud-run needs --composite-job")?,
                client: reqwest::Client::new(),
            },
        })
    }

    /// `args` are the job's arguments after its mode.
    pub(crate) async fn launch(&self, job: Job, args: Vec<String>) -> Result<()> {
        let mut command = vec![job.mode().to_string()];
        command.extend(args);
        match self {
            Self::Local { program } => {
                let mut child = tokio::process::Command::new(program)
                    .args(&command)
                    .spawn()
                    .with_context(|| format!("cannot start {}", program.display()))?;
                tokio::spawn(async move {
                    match child.wait().await {
                        Ok(status) => tracing::info!(mode = job.mode(), %status, "job exited"),
                        Err(error) => {
                            tracing::warn!(mode = job.mode(), %error, "cannot wait for the job")
                        }
                    }
                });
            }
            Self::CloudRun {
                record_job,
                composite_job,
                client,
            } => {
                let name = match job {
                    Job::Record => record_job,
                    Job::Composite => composite_job,
                };
                run_cloud_run_job(client, name, command).await?;
            }
        }
        tracing::info!(mode = job.mode(), "job launched");
        Ok(())
    }
}

async fn run_cloud_run_job(client: &reqwest::Client, job: &str, args: Vec<String>) -> Result<()> {
    let token: Value = client
        .get(METADATA_TOKEN_URL)
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .context("cannot get a token from the metadata server")?
        .json()
        .await?;
    let token = token["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("the metadata server returned no access_token"))?;
    let response = client
        .post(format!("https://run.googleapis.com/v2/{job}:run"))
        .bearer_auth(token)
        .json(&json!({ "overrides": { "containerOverrides": [{ "args": args }] } }))
        .send()
        .await
        .with_context(|| format!("cannot run {job}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("running {job} answered {status}: {body}");
    }
    Ok(())
}
