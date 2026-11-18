use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use google_cloud::MetadataToken;
use serde_json::json;

/// Starts one job without waiting for it: as a child process locally, or as
/// an execution of a Cloud Run Job whose container arguments are replaced by
/// the given ones.
#[derive(Clone)]
pub enum JobLauncher {
    Local {
        program: PathBuf,
    },
    CloudRun {
        /// `projects/<project>/locations/<region>/jobs/<job>`
        job: String,
        client: reqwest::Client,
        /// Its service account needs `run.jobs.runWithOverrides` on `job`.
        metadata_token: Arc<MetadataToken>,
    },
}

impl JobLauncher {
    /// `local_env` is set on a local child only: a Cloud Run execution's
    /// overrides are visible to whoever can view the execution, so they never
    /// carry it.
    /// Fails when the child process cannot be spawned, the metadata server
    /// does not answer, or the Cloud Run Admin API refuses the execution.
    pub async fn launch(&self, args: Vec<String>, local_env: Vec<(String, String)>) -> Result<()> {
        match self {
            Self::Local { program } => {
                let mut child = tokio::process::Command::new(program)
                    .args(&args)
                    .envs(local_env)
                    .spawn()
                    .with_context(|| format!("cannot start {}", program.display()))?;
                tokio::spawn(async move {
                    match child.wait().await {
                        Ok(status) => tracing::info!(%status, "job exited"),
                        Err(error) => tracing::warn!(%error, "cannot wait for the job"),
                    }
                });
            }
            Self::CloudRun {
                job,
                client,
                metadata_token,
            } => {
                let token = metadata_token
                    .bearer_token()
                    .await
                    .context("cannot get a token from the metadata server")?;
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
            }
        }
        Ok(())
    }
}
