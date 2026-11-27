use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use tokio::{sync::Mutex, time::Instant};

const METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const REFRESH_BEFORE_EXPIRY: Duration = Duration::from_secs(60);
/// The metadata server is on the instance's own network; a request that takes
/// this long is stuck.
const METADATA_TIMEOUT: Duration = Duration::from_secs(5);

/// The OAuth access token of the service account Cloud Run runs the service
/// as, read from the metadata server and kept until a minute before it
/// expires. Fails outside Google Cloud, where there is no metadata server.
pub(crate) struct MetadataToken {
    http: Client,
    cached: Mutex<Option<AccessToken>>,
}

struct AccessToken {
    value: String,
    refresh_at: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl MetadataToken {
    pub(crate) fn new() -> Self {
        Self {
            http: Client::new(),
            cached: Mutex::new(None),
        }
    }

    pub(crate) async fn bearer_token(&self) -> anyhow::Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref()
            && Instant::now() < token.refresh_at
        {
            return Ok(token.value.clone());
        }
        let response: TokenResponse = self
            .http
            .get(METADATA_TOKEN_URL)
            .timeout(METADATA_TIMEOUT)
            .header("Metadata-Flavor", "Google")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let token = AccessToken {
            value: response.access_token,
            refresh_at: Instant::now()
                + Duration::from_secs(response.expires_in).saturating_sub(REFRESH_BEFORE_EXPIRY),
        };
        let value = token.value.clone();
        *cached = Some(token);
        Ok(value)
    }
}
