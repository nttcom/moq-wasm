use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

const METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const REFRESH_BEFORE_EXPIRY: Duration = Duration::from_secs(60);

/// The OAuth access token of the service account a Google Cloud workload runs
/// as (Cloud Run, Compute Engine), read from the metadata server and kept until
/// a minute before it expires. Fails outside Google Cloud, where there is no
/// metadata server.
pub struct MetadataToken {
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
    pub fn new(http: Client) -> Self {
        Self {
            http,
            cached: Mutex::new(None),
        }
    }

    pub async fn bearer_token(&self) -> anyhow::Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref()
            && Instant::now() < token.refresh_at
        {
            return Ok(token.value.clone());
        }
        let response: TokenResponse = self
            .http
            .get(METADATA_TOKEN_URL)
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
