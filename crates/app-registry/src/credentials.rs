use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

const METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";
const REFRESH_BEFORE_EXPIRY: Duration = Duration::from_secs(60);

pub(crate) enum Credentials {
    None,
    MetadataServer(Mutex<Option<AccessToken>>),
}

pub(crate) struct AccessToken {
    value: String,
    refresh_at: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl Credentials {
    pub(crate) async fn bearer_token(&self, http: &Client) -> anyhow::Result<Option<String>> {
        let Self::MetadataServer(cached) = self else {
            return Ok(None);
        };
        let mut cached = cached.lock().await;
        if let Some(token) = cached.as_ref()
            && Instant::now() < token.refresh_at
        {
            return Ok(Some(token.value.clone()));
        }
        let response: TokenResponse = http
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
        Ok(Some(value))
    }
}
