use anyhow::{Context, Result, bail};
use metadata_token::MetadataToken;
use tokio::process::Command;

const MODEL_ID: &str = "gemini-3.5-live-translate-preview";
const BIDI_PATH: &str = "/ws/google.cloud.aiplatform.v1beta1.LlmBidiService/BidiGenerateContent";
const GLOBAL_LOCATION: &str = "global";

/// Where, and as whom, the Agent Platform's Live API is called. The
/// translate model is served from the `global` location only (2026-11).
pub(crate) struct AgentPlatform {
    pub(crate) endpoint: String,
    pub(crate) model: String,
    credentials: Credentials,
}

/// On Cloud Run the service account's token comes from the metadata server.
/// Locally a developer names the account and `gcloud` impersonates it for a
/// fresh token per connection.
pub(crate) enum Credentials {
    Gcloud { service_account: String },
    MetadataServer(MetadataToken),
}

impl AgentPlatform {
    pub(crate) fn new(project: &str, location: &str, credentials: Credentials) -> Self {
        Self {
            endpoint: format!("wss://{}{BIDI_PATH}", host(location)),
            model: format!(
                "projects/{project}/locations/{location}/publishers/google/models/{MODEL_ID}"
            ),
            credentials,
        }
    }

    pub(crate) async fn bearer_token(&self) -> Result<String> {
        match &self.credentials {
            Credentials::Gcloud { service_account } => gcloud_access_token(service_account).await,
            Credentials::MetadataServer(token) => token.bearer_token().await,
        }
    }

    pub(crate) fn credentials_source(&self) -> &'static str {
        match self.credentials {
            Credentials::Gcloud { .. } => "gcloud",
            Credentials::MetadataServer(_) => "metadata-server",
        }
    }
}

/// Needs `gcloud` on the PATH, logged in as a user with
/// `roles/iam.serviceAccountTokenCreator` on the account.
async fn gcloud_access_token(service_account: &str) -> Result<String> {
    let output = Command::new("gcloud")
        .args([
            "auth",
            "print-access-token",
            "--impersonate-service-account",
            service_account,
        ])
        .output()
        .await
        .context("run gcloud auth print-access-token")?;
    if !output.status.success() {
        bail!(
            "gcloud auth print-access-token failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let token = String::from_utf8(output.stdout)
        .context("gcloud printed a non-UTF-8 token")?
        .trim()
        .to_string();
    if token.is_empty() {
        bail!("gcloud printed an empty access token");
    }
    Ok(token)
}

fn host(location: &str) -> String {
    if location == GLOBAL_LOCATION {
        "aiplatform.googleapis.com".to_string()
    } else {
        format!("{location}-aiplatform.googleapis.com")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_credentials() -> Credentials {
        Credentials::Gcloud {
            service_account: "sa@example.iam.gserviceaccount.com".to_string(),
        }
    }

    #[test]
    fn the_global_location_dials_the_host_without_a_region_prefix() {
        // Act
        let platform = AgentPlatform::new("demo-project", "global", test_credentials());

        // Assert
        assert_eq!(
            platform.endpoint,
            "wss://aiplatform.googleapis.com/ws/google.cloud.aiplatform.v1beta1.LlmBidiService/BidiGenerateContent"
        );
        assert_eq!(
            platform.model,
            "projects/demo-project/locations/global/publishers/google/models/gemini-3.5-live-translate-preview"
        );
    }

    #[test]
    fn a_regional_location_dials_its_regional_host() {
        // Act
        let platform = AgentPlatform::new("demo-project", "us-central1", test_credentials());

        // Assert
        assert!(
            platform
                .endpoint
                .starts_with("wss://us-central1-aiplatform.googleapis.com/")
        );
        assert!(platform.model.contains("/locations/us-central1/"));
    }
}
