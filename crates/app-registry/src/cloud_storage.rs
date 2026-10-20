use anyhow::{Context, bail};
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url, header::CONTENT_TYPE};
use tokio::sync::Mutex;

use crate::{
    app::Apps,
    credentials::Credentials,
    store::{Generation, Snapshot, Stored},
};

const CLOUD_STORAGE_URL: &str = "https://storage.googleapis.com/";
const EMULATOR_HOST_ENV: &str = "STORAGE_EMULATOR_HOST";
const GENERATION_HEADER: &str = "x-goog-generation";

pub enum Endpoint {
    CloudStorage,
    Emulator(Url),
}

impl Endpoint {
    /// Uses the emulator at `STORAGE_EMULATOR_HOST` when it is set and non-empty, Cloud Storage
    /// otherwise. Fails when the value is not a URL such as `http://localhost:4443`.
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var(EMULATOR_HOST_ENV) {
            Ok(host) if !host.is_empty() => {
                Ok(Self::Emulator(Url::parse(&host).with_context(|| {
                    format!("{EMULATOR_HOST_ENV} is not a URL")
                })?))
            }
            _ => Ok(Self::CloudStorage),
        }
    }
}

pub(crate) struct CloudStorageObject {
    http: Client,
    object_url: Url,
    upload_url: Url,
    credentials: Credentials,
}

impl CloudStorageObject {
    pub(crate) fn new(endpoint: Endpoint, bucket: &str, object: &str) -> anyhow::Result<Self> {
        let (base, credentials) = match endpoint {
            Endpoint::CloudStorage => (
                Url::parse(CLOUD_STORAGE_URL).expect("CLOUD_STORAGE_URL is a valid URL"),
                Credentials::MetadataServer(Mutex::new(None)),
            ),
            Endpoint::Emulator(url) => (url, Credentials::None),
        };
        if !matches!(base.scheme(), "http" | "https") {
            bail!("the Cloud Storage endpoint {base} is not an http(s) URL");
        }
        let mut object_url = base.clone();
        object_url
            .path_segments_mut()
            .expect("an http(s) URL has path segments")
            .pop_if_empty()
            .extend(["storage", "v1", "b", bucket, "o", object]);
        let mut upload_url = base;
        upload_url
            .path_segments_mut()
            .expect("an http(s) URL has path segments")
            .pop_if_empty()
            .extend(["upload", "storage", "v1", "b", bucket, "o"]);
        upload_url
            .query_pairs_mut()
            .append_pair("uploadType", "media")
            .append_pair("name", object);
        Ok(Self {
            http: Client::new(),
            object_url,
            upload_url,
            credentials,
        })
    }

    pub(crate) async fn load(&self) -> anyhow::Result<Snapshot> {
        let mut url = self.object_url.clone();
        url.query_pairs_mut().append_pair("alt", "media");
        let response = self.send(self.http.get(url)).await?;
        let generation = response
            .headers()
            .get(GENERATION_HEADER)
            .context("the registry response has no generation")?
            .to_str()?
            .parse()
            .context("the registry generation is not a number")?;
        let apps = Apps::parse(&response.text().await?)?;
        Ok(Snapshot {
            apps,
            generation: Generation(generation),
        })
    }

    pub(crate) async fn store(
        &self,
        apps: &Apps,
        if_generation_match: Generation,
    ) -> anyhow::Result<Stored> {
        let mut url = self.upload_url.clone();
        url.query_pairs_mut()
            .append_pair("ifGenerationMatch", &if_generation_match.0.to_string());
        let request = self
            .authorize(self.http.post(url))
            .await?
            .header(CONTENT_TYPE, "application/json")
            .body(apps.to_json()?);
        let response = request.send().await?;
        match response.status() {
            StatusCode::PRECONDITION_FAILED => Ok(Stored::Conflict),
            status if status.is_success() => Ok(Stored::Written),
            status => bail!(
                "storing the registry failed with {status}: {}",
                response.text().await.unwrap_or_default()
            ),
        }
    }

    async fn send(&self, request: RequestBuilder) -> anyhow::Result<Response> {
        let response = self.authorize(request).await?.send().await?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "reading the registry failed with {status}: {}",
                response.text().await.unwrap_or_default()
            );
        }
        Ok(response)
    }

    async fn authorize(&self, request: RequestBuilder) -> anyhow::Result<RequestBuilder> {
        Ok(match self.credentials.bearer_token(&self.http).await? {
            Some(token) => request.bearer_auth(token),
            None => request,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        App,
        test_support::{FakeCloudStorage, app},
    };

    #[test]
    fn new_rejects_an_emulator_endpoint_without_an_http_scheme() {
        // Arrange
        let endpoint = Endpoint::Emulator(Url::parse("localhost:4443").unwrap());

        // Act
        let error = CloudStorageObject::new(endpoint, "bucket", "registry.json")
            .err()
            .unwrap();

        // Assert
        assert!(error.to_string().contains("http(s)"), "{error}");
    }

    #[tokio::test]
    async fn load_returns_the_apps_and_the_generation_of_the_object() {
        // Arrange
        let storage = FakeCloudStorage::run().await;
        storage.put(&json!([{ "appId": "a", "secret": "s", "isRelay": false }]));

        // Act
        let snapshot = storage.store().load().await.unwrap();

        // Assert
        assert_eq!(
            snapshot.apps,
            Apps::new(vec![App {
                secret: "s".into(),
                ..app("a", None)
            }])
            .unwrap()
        );
        assert_eq!(snapshot.generation, Generation(1));
    }

    #[tokio::test]
    async fn load_fails_when_the_object_does_not_exist() {
        // Arrange
        let storage = FakeCloudStorage::run().await;

        // Act
        let error = storage.store().load().await.unwrap_err();

        // Assert
        assert!(error.to_string().contains("404"), "{error}");
    }
}
