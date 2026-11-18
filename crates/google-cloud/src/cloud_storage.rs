use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::{Client, RequestBuilder, StatusCode, Url, header::CONTENT_TYPE};

use crate::metadata_token::MetadataToken;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versioned {
    pub body: Vec<u8>,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uploaded {
    Written,
    Conflict,
}

/// Requests to Cloud Storage carry the workload's `MetadataToken`, so they
/// fail outside Google Cloud. Requests to an emulator carry no token.
pub struct CloudStorageBucket {
    http: Client,
    objects_url: Url,
    upload_url: Url,
    metadata_token: Option<MetadataToken>,
}

impl CloudStorageBucket {
    /// Fails when the emulator URL is not http(s).
    pub fn new(
        endpoint: Endpoint,
        bucket: &str,
        request_timeout: Duration,
    ) -> anyhow::Result<Self> {
        let http = Client::builder().timeout(request_timeout).build()?;
        let (base, metadata_token) = match endpoint {
            Endpoint::CloudStorage => (
                Url::parse(CLOUD_STORAGE_URL).expect("CLOUD_STORAGE_URL is a valid URL"),
                Some(MetadataToken::new(http.clone())),
            ),
            Endpoint::Emulator(url) => (url, None),
        };
        if !matches!(base.scheme(), "http" | "https") {
            bail!("the Cloud Storage endpoint {base} is not an http(s) URL");
        }
        let mut objects_url = base.clone();
        objects_url
            .path_segments_mut()
            .expect("an http(s) URL has path segments")
            .pop_if_empty()
            .extend(["storage", "v1", "b", bucket, "o"]);
        let mut upload_url = base;
        upload_url
            .path_segments_mut()
            .expect("an http(s) URL has path segments")
            .pop_if_empty()
            .extend(["upload", "storage", "v1", "b", bucket, "o"]);
        Ok(Self {
            http,
            objects_url,
            upload_url,
            metadata_token,
        })
    }

    /// Fails when the object does not exist.
    pub async fn download(&self, object: &str) -> anyhow::Result<Versioned> {
        let mut url = self.object_url(object);
        url.query_pairs_mut().append_pair("alt", "media");
        let response = self.authorize(self.http.get(url)).await?.send().await?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "downloading {object} failed with {status}: {}",
                response.text().await.unwrap_or_default()
            );
        }
        let generation = response
            .headers()
            .get(GENERATION_HEADER)
            .with_context(|| format!("the response for {object} has no generation"))?
            .to_str()?
            .parse()
            .with_context(|| format!("the generation of {object} is not a number"))?;
        Ok(Versioned {
            body: response.bytes().await?.to_vec(),
            generation,
        })
    }

    /// Generation 0 writes the object only if it does not exist. Returns
    /// `Uploaded::Conflict` when the object is at another generation.
    pub async fn upload_if_generation_matches(
        &self,
        object: &str,
        content_type: &str,
        body: Vec<u8>,
        generation: u64,
    ) -> anyhow::Result<Uploaded> {
        let mut url = self.upload_url.clone();
        url.query_pairs_mut()
            .append_pair("uploadType", "media")
            .append_pair("name", object)
            .append_pair("ifGenerationMatch", &generation.to_string());
        let request = self
            .authorize(self.http.post(url))
            .await?
            .header(CONTENT_TYPE, content_type)
            .body(body);
        let response = request.send().await?;
        match response.status() {
            StatusCode::PRECONDITION_FAILED => Ok(Uploaded::Conflict),
            status if status.is_success() => Ok(Uploaded::Written),
            status => bail!(
                "uploading {object} failed with {status}: {}",
                response.text().await.unwrap_or_default()
            ),
        }
    }

    fn object_url(&self, object: &str) -> Url {
        let mut url = self.objects_url.clone();
        url.path_segments_mut()
            .expect("an http(s) URL has path segments")
            .push(object);
        url
    }

    async fn authorize(&self, request: RequestBuilder) -> anyhow::Result<RequestBuilder> {
        Ok(match &self.metadata_token {
            Some(token) => request.bearer_auth(token.bearer_token().await?),
            None => request,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeCloudStorage, SilentServer};

    const BUCKET: &str = "bucket";
    const TIMEOUT: Duration = Duration::from_secs(2);

    #[test]
    fn new_rejects_an_emulator_endpoint_without_an_http_scheme() {
        // Arrange
        let endpoint = Endpoint::Emulator(Url::parse("localhost:4443").unwrap());

        // Act
        let error = CloudStorageBucket::new(endpoint, BUCKET, TIMEOUT)
            .err()
            .unwrap();

        // Assert
        assert!(error.to_string().contains("http(s)"), "{error}");
    }

    #[tokio::test]
    async fn download_returns_the_body_and_the_generation_of_the_object() {
        // Arrange
        let storage = FakeCloudStorage::run(BUCKET).await;
        storage.put("registry.json", "first");
        storage.put("registry.json", "second");

        // Act
        let object = storage.bucket().download("registry.json").await.unwrap();

        // Assert
        assert_eq!(
            object,
            Versioned {
                body: b"second".to_vec(),
                generation: 2,
            }
        );
    }

    #[tokio::test]
    async fn download_fails_when_the_object_does_not_exist() {
        // Arrange
        let storage = FakeCloudStorage::run(BUCKET).await;

        // Act
        let error = storage.bucket().download("missing").await.unwrap_err();

        // Assert
        assert!(error.to_string().contains("404"), "{error}");
    }

    #[tokio::test]
    async fn download_gives_up_within_the_request_timeout_on_a_server_that_never_answers() {
        // Arrange
        let server = SilentServer::run().await;
        let bucket = CloudStorageBucket::new(server.endpoint(), BUCKET, TIMEOUT).unwrap();

        // Act
        let result =
            tokio::time::timeout(TIMEOUT + Duration::from_secs(1), bucket.download("object")).await;

        // Assert
        assert!(
            result
                .expect("download must not wait past its timeout")
                .is_err()
        );
    }

    #[tokio::test]
    async fn upload_writes_only_on_the_generation_it_was_made_on() {
        // Arrange
        let storage = FakeCloudStorage::run(BUCKET).await;
        storage.put("registry.json", "[]");
        let bucket = storage.bucket();

        // Act
        let stale = bucket
            .upload_if_generation_matches("registry.json", "application/json", b"stale".into(), 0)
            .await
            .unwrap();
        let current = bucket
            .upload_if_generation_matches("registry.json", "application/json", b"new".into(), 1)
            .await
            .unwrap();

        // Assert
        assert_eq!(stale, Uploaded::Conflict);
        assert_eq!(current, Uploaded::Written);
        assert_eq!(storage.body("registry.json").unwrap(), b"new");
        assert_eq!(storage.generation("registry.json"), Some(2));
    }
}
