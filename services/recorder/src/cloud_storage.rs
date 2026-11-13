use std::time::Duration;

use anyhow::{Result, bail};
use metadata_token::MetadataToken;
use reqwest::{Client, StatusCode};
use url::Url;

const CLOUD_STORAGE_URL: &str = "https://storage.googleapis.com/";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The objects of a Cloud Storage bucket, reached through the JSON API for
/// the conditional delete `object_store` lacks. Authorized as the service
/// account of the Google Cloud workload, so it fails outside Google Cloud.
pub(crate) struct CloudStorageBucket {
    http: Client,
    bucket_url: Url,
    metadata_token: Option<MetadataToken>,
}

impl CloudStorageBucket {
    pub(crate) fn new(bucket: &str) -> Result<Self> {
        let http = Client::builder().timeout(REQUEST_TIMEOUT).build()?;
        let base = Url::parse(CLOUD_STORAGE_URL).expect("CLOUD_STORAGE_URL is a valid URL");
        let metadata_token = Some(MetadataToken::new(http.clone()));
        Ok(Self::at(http, base, bucket, metadata_token))
    }

    fn at(
        http: Client,
        mut base: Url,
        bucket: &str,
        metadata_token: Option<MetadataToken>,
    ) -> Self {
        base.path_segments_mut()
            .expect("an http(s) URL has path segments")
            .pop_if_empty()
            .extend(["storage", "v1", "b", bucket, "o"]);
        Self {
            http,
            bucket_url: base,
            metadata_token,
        }
    }

    /// An object replaced or deleted since is not an error. `object` is the
    /// name `object_store` stores it under (`Path::as_ref`).
    pub(crate) async fn delete_if_generation_matches(
        &self,
        object: &str,
        generation: &str,
    ) -> Result<()> {
        let mut url = self.bucket_url.clone();
        url.path_segments_mut()
            .expect("an http(s) URL has path segments")
            .push(object);
        url.query_pairs_mut()
            .append_pair("ifGenerationMatch", generation);
        let mut request = self.http.delete(url);
        if let Some(token) = &self.metadata_token {
            request = request.bearer_auth(token.bearer_token().await?);
        }
        let response = request.send().await?;
        match response.status() {
            StatusCode::PRECONDITION_FAILED | StatusCode::NOT_FOUND => Ok(()),
            status if status.is_success() => Ok(()),
            status => bail!(
                "deleting {object} failed with {status}: {}",
                response.text().await.unwrap_or_default()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use http_body_util::Full;
    use hyper::{Response, body::Bytes, server::conn::http1, service::service_fn};
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;

    use super::*;

    /// Records each request as `<method> <path>?<query>`, as sent on the wire.
    async fn answering(status: u16) -> (CloudStorageBucket, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let service = service_fn(move |request: hyper::Request<_>| {
                        recorded.lock().unwrap().push(format!(
                            "{} {}",
                            request.method(),
                            request.uri()
                        ));
                        async move {
                            Ok::<_, hyper::Error>(
                                Response::builder()
                                    .status(status)
                                    .body(Full::new(Bytes::new()))
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let bucket = CloudStorageBucket::at(Client::new(), base, "bucket", None);
        (bucket, requests)
    }

    #[tokio::test]
    async fn names_the_object_as_object_store_stores_it_with_the_generation() {
        // Arrange: object_store keeps the `/` of a namespace prefix escaped in the object name
        let (bucket, requests) = answering(204).await;

        // Act
        bucket
            .delete_if_generation_matches("recorder/locks/anon%2Fresearch", "1700000000000001")
            .await
            .unwrap();

        // Assert
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            [
                "DELETE /storage/v1/b/bucket/o/recorder%2Flocks%2Fanon%252Fresearch?ifGenerationMatch=1700000000000001"
            ]
        );
    }

    #[tokio::test]
    async fn takes_a_replaced_or_missing_object_as_done_and_fails_on_an_error() {
        // Arrange
        let (replaced, _) = answering(412).await;
        let (missing, _) = answering(404).await;
        let (failing, _) = answering(503).await;

        // Act
        let replaced = replaced.delete_if_generation_matches("lock", "1").await;
        let missing = missing.delete_if_generation_matches("lock", "1").await;
        let failing = failing.delete_if_generation_matches("lock", "1").await;

        // Assert
        assert!(replaced.is_ok());
        assert!(missing.is_ok());
        assert!(failing.unwrap_err().to_string().contains("503"));
    }
}
