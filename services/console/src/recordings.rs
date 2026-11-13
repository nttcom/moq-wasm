use std::time::Duration;

use anyhow::Context;
use hyper::body::Bytes;
use reqwest::{StatusCode, Url};
use serde::Deserialize;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Reads an app's recordings from the recorder with a token the console
/// signs for the app, so the recorder answers that app alone.
pub(crate) struct RecorderClient {
    http: reqwest::Client,
    base_url: Url,
}

#[derive(Deserialize)]
struct FileUrl {
    url: String,
}

impl RecorderClient {
    pub(crate) fn new(base_url: &str) -> anyhow::Result<Self> {
        let base_url = Url::parse(base_url).context("CONSOLE_RECORDER_URL is not a URL")?;
        anyhow::ensure!(
            !base_url.cannot_be_a_base(),
            "CONSOLE_RECORDER_URL is not an http(s) URL"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            base_url,
        })
    }

    /// The recorder's JSON, passed to the page as is.
    pub(crate) async fn recordings(&self, token: &str, app_id: &str) -> anyhow::Result<Bytes> {
        let response = self
            .http
            .get(self.url(&["apps", app_id, "recordings"]))
            .bearer_auth(token)
            .send()
            .await
            .context("the recorder is unreachable")?;
        match response.status() {
            StatusCode::OK => Ok(response.bytes().await?),
            status => anyhow::bail!("the recorder answered {status}"),
        }
    }

    /// The signed URL that reads file `name` of recording `id` from the
    /// store; `None` when the app has no such recording or file.
    pub(crate) async fn file_url(
        &self,
        token: &str,
        id: &str,
        name: &str,
    ) -> anyhow::Result<Option<Url>> {
        let mut url = self.url(&["recordings", id, "file"]);
        url.query_pairs_mut().append_pair("name", name);
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .context("the recorder is unreachable")?;
        match response.status() {
            StatusCode::OK => {
                let answer: FileUrl = response.json().await?;
                Ok(Some(
                    Url::parse(&answer.url).context("the recorder answered no URL")?,
                ))
            }
            // A recording of another app is outside the token's claim.
            StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => Ok(None),
            status => anyhow::bail!("the recorder answered {status}"),
        }
    }

    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .expect("checked in new")
            .pop_if_empty()
            .extend(segments);
        url
    }
}
