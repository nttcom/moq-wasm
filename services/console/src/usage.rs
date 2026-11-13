use std::time::Duration;

use anyhow::Context;
use hyper::body::Bytes;
use reqwest::{StatusCode, Url};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The query parameters of `GET /api/apps/{appId}/usage` passed on to the
/// observability server, which validates them.
const RANGE_PARAMETERS: [&str; 3] = ["from", "to", "points"];

pub(crate) enum UsageAnswer {
    /// The observability server's JSON, passed to the page as is.
    Usage(Bytes),
    /// The observability server refused the range.
    InvalidRange,
}

/// Reads an app's traffic from the observability server's `GET /api/usage`
/// with a token the console signs for the app, so the server answers that
/// app alone.
pub(crate) struct ObservabilityClient {
    http: reqwest::Client,
    usage_url: Url,
}

impl ObservabilityClient {
    pub(crate) fn new(base_url: &str) -> anyhow::Result<Self> {
        let usage_url = Url::parse(base_url)
            .context("CONSOLE_OBSERVABILITY_URL is not a URL")?
            .join("/api/usage")?;
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            usage_url,
        })
    }

    pub(crate) async fn usage(
        &self,
        token: &str,
        query: Option<&str>,
    ) -> anyhow::Result<UsageAnswer> {
        let given = Url::parse(&format!("http://query/?{}", query.unwrap_or_default()))?;
        let mut url = self.usage_url.clone();
        url.query_pairs_mut().extend_pairs(
            given
                .query_pairs()
                .filter(|(name, _)| RANGE_PARAMETERS.contains(&name.as_ref())),
        );
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .context("the observability server is unreachable")?;
        match response.status() {
            StatusCode::OK => Ok(UsageAnswer::Usage(response.bytes().await?)),
            StatusCode::BAD_REQUEST => Ok(UsageAnswer::InvalidRange),
            status => anyhow::bail!("the observability server answered {status}"),
        }
    }
}
