use anyhow::bail;

use crate::{
    app::{App, Apps},
    cloud_storage::CloudStorageObject,
    file::RegistryFile,
    location::Location,
};

const MAX_UPDATE_ATTEMPTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation(pub(crate) u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub apps: Apps,
    pub generation: Generation,
}

pub struct RegistryStore {
    backend: Backend,
}

enum Backend {
    CloudStorage(Box<CloudStorageObject>),
    File(RegistryFile),
}

pub(crate) enum Stored {
    Written,
    Conflict,
}

impl RegistryStore {
    pub fn new(location: Location) -> anyhow::Result<Self> {
        let backend = match location {
            Location::CloudStorage {
                endpoint,
                bucket,
                object,
            } => Backend::CloudStorage(Box::new(CloudStorageObject::new(
                endpoint, &bucket, &object,
            )?)),
            Location::File(path) => Backend::File(RegistryFile::new(path)),
        };
        Ok(Self { backend })
    }

    pub async fn load(&self) -> anyhow::Result<Snapshot> {
        match &self.backend {
            Backend::CloudStorage(object) => object.load().await,
            Backend::File(file) => file.load().await,
        }
    }

    /// Runs `change` on the latest apps and stores the result only if `change` altered them and no
    /// other writer stored the registry in between. When another writer did, it reloads and runs
    /// `change` again, at most five runs in total, so `change` must have no side effects beyond the
    /// apps it is given. Fails without storing when the registry does not exist, when `change` fails,
    /// when the changed apps break the rules `Apps::new` checks, or when all five runs conflict.
    pub async fn update<T>(
        &self,
        mut change: impl FnMut(&mut Vec<App>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        for _ in 0..MAX_UPDATE_ATTEMPTS {
            let snapshot = self.load().await?;
            let mut apps = snapshot.apps.clone().into_vec();
            let outcome = change(&mut apps)?;
            let apps = Apps::new(apps)?;
            if apps == snapshot.apps {
                return Ok(outcome);
            }
            match self.store(&apps, snapshot.generation).await? {
                Stored::Written => return Ok(outcome),
                Stored::Conflict => continue,
            }
        }
        bail!("another writer kept changing the registry during {MAX_UPDATE_ATTEMPTS} attempts")
    }

    async fn store(&self, apps: &Apps, if_generation_match: Generation) -> anyhow::Result<Stored> {
        match &self.backend {
            Backend::CloudStorage(object) => object.store(apps, if_generation_match).await,
            Backend::File(file) => file.store(apps, if_generation_match).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::test_support::{FakeCloudStorage, app};

    #[tokio::test]
    async fn update_stores_the_changed_apps_and_returns_what_the_change_returned() {
        // Arrange
        let storage = FakeCloudStorage::run().await;
        storage.put(&json!([]));

        // Act
        let outcome = storage
            .store()
            .update(|apps| {
                apps.push(app("a", Some("sub-1")));
                Ok("created")
            })
            .await
            .unwrap();

        // Assert
        assert_eq!(outcome, "created");
        assert_eq!(
            storage.body(),
            json!([{ "appId": "a", "secret": "a-secret", "isRelay": false, "sub": "sub-1" }])
        );
    }

    #[tokio::test]
    async fn update_runs_the_change_again_on_top_of_a_write_made_in_between() {
        // Arrange
        let storage = FakeCloudStorage::run().await;
        storage.put(&json!([]));
        let mut attempts = 0;

        // Act
        storage
            .store()
            .update(|apps| {
                attempts += 1;
                if attempts == 1 {
                    storage.put(&json!([{ "appId": "other", "secret": "s", "isRelay": false }]));
                }
                apps.push(app("a", Some("sub-1")));
                Ok(())
            })
            .await
            .unwrap();

        // Assert
        assert_eq!(attempts, 2);
        assert_eq!(
            storage.body(),
            json!([
                { "appId": "other", "secret": "s", "isRelay": false },
                { "appId": "a", "secret": "a-secret", "isRelay": false, "sub": "sub-1" },
            ])
        );
    }

    #[tokio::test]
    async fn update_does_not_store_when_the_change_leaves_the_apps_alone() {
        // Arrange
        let storage = FakeCloudStorage::run().await;
        storage.put(&json!([{ "appId": "a", "secret": "s", "isRelay": false }]));

        // Act
        let outcome = storage.store().update(|_| Ok("unchanged")).await.unwrap();

        // Assert
        assert_eq!(outcome, "unchanged");
        assert_eq!(storage.generation(), 1);
    }

    #[tokio::test]
    async fn update_leaves_the_registry_alone_when_the_change_breaks_it() {
        // Arrange
        let storage = FakeCloudStorage::run().await;
        storage.put(&json!([{ "appId": "a", "secret": "s", "isRelay": false, "sub": "sub-1" }]));

        // Act
        let error = storage
            .store()
            .update(|apps| {
                apps.push(app("a", Some("sub-2")));
                Ok(())
            })
            .await
            .unwrap_err();

        // Assert
        assert!(error.to_string().contains("duplicated"), "{error}");
        assert_eq!(
            storage.body(),
            json!([{ "appId": "a", "secret": "s", "isRelay": false, "sub": "sub-1" }])
        );
    }
}
