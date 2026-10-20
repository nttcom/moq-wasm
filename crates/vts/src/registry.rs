use std::sync::{Arc, Mutex, MutexGuard};

use app_registry::{Apps, RegistryStore};

pub struct Registry {
    store: RegistryStore,
    last_loaded: Mutex<Arc<Apps>>,
}

impl Registry {
    pub async fn load(store: RegistryStore) -> anyhow::Result<Self> {
        let apps = store.load().await?.apps;
        tracing::info!(apps = apps.len(), "registry loaded");
        Ok(Self {
            store,
            last_loaded: Mutex::new(Arc::new(apps)),
        })
    }

    pub(crate) async fn current(&self) -> Arc<Apps> {
        match self.store.load().await {
            Ok(snapshot) => {
                let apps = Arc::new(snapshot.apps);
                *self.last_loaded() = apps.clone();
                apps
            }
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "registry read failed; verifying with the last loaded registry"
                );
                self.last_loaded().clone()
            }
        }
    }

    fn last_loaded(&self) -> MutexGuard<'_, Arc<Apps>> {
        self.last_loaded
            .lock()
            .expect("no holder of last_loaded panics while holding it")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support::TemporaryRegistryFile;

    #[tokio::test]
    async fn current_reads_the_registry_again_on_every_call() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        file.write(&json!([{ "appId": "a", "secret": "old", "isRelay": false }]).to_string());
        let registry = Registry::load(file.store()).await.unwrap();

        // Act
        file.write(&json!([{ "appId": "a", "secret": "new", "isRelay": false }]).to_string());

        // Assert
        assert_eq!(registry.current().await.get("a").unwrap().secret, "new");
    }

    #[tokio::test]
    async fn current_falls_back_to_the_last_loaded_registry_when_reading_fails() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        file.write(&json!([{ "appId": "a", "secret": "old", "isRelay": false }]).to_string());
        let registry = Registry::load(file.store()).await.unwrap();

        // Act
        file.write("not a registry");

        // Assert
        assert_eq!(registry.current().await.get("a").unwrap().secret, "old");
    }
}
