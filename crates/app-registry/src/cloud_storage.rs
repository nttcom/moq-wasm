use std::time::Duration;

use google_cloud::{CloudStorageBucket, Endpoint, Uploaded};

use crate::{
    app::Apps,
    store::{Generation, Snapshot},
};

// Shorter than the 3 seconds the relay waits for the VTS, so the VTS can still answer from the
// registry it read last.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct CloudStorageObject {
    bucket: CloudStorageBucket,
    object: String,
}

impl CloudStorageObject {
    pub(crate) fn new(endpoint: Endpoint, bucket: &str, object: &str) -> anyhow::Result<Self> {
        Ok(Self {
            bucket: CloudStorageBucket::new(endpoint, bucket, REQUEST_TIMEOUT)?,
            object: object.to_owned(),
        })
    }

    pub(crate) async fn load(&self) -> anyhow::Result<Snapshot> {
        let object = self.bucket.download(&self.object).await?;
        Ok(Snapshot {
            apps: Apps::parse(std::str::from_utf8(&object.body)?)?,
            generation: Generation(object.generation),
        })
    }

    pub(crate) async fn store(
        &self,
        apps: &Apps,
        if_generation_match: Generation,
    ) -> anyhow::Result<Uploaded> {
        self.bucket
            .upload_if_generation_matches(
                &self.object,
                "application/json",
                apps.to_json()?,
                if_generation_match.0,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{
        App, Apps,
        store::Generation,
        test_support::{FakeRegistry, app},
    };

    #[tokio::test]
    async fn load_returns_the_apps_and_the_generation_of_the_object() {
        // Arrange
        let registry = FakeRegistry::run().await;
        registry.put(&json!([{ "appId": "a", "secret": "s", "isRelay": false }]));

        // Act
        let snapshot = registry.store().load().await.unwrap();

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
}
