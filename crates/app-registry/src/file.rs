use std::{
    ffi::OsString,
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
};

use anyhow::Context;

use crate::{
    app::Apps,
    store::{Generation, Snapshot, Stored},
};

pub(crate) struct RegistryFile {
    path: PathBuf,
}

impl RegistryFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) async fn load(&self) -> anyhow::Result<Snapshot> {
        let bytes = self.read().await?;
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("the registry file {} is not UTF-8", self.path.display()))?;
        Ok(Snapshot {
            apps: Apps::parse(text)?,
            generation: generation_of(&bytes),
        })
    }

    pub(crate) async fn store(
        &self,
        apps: &Apps,
        if_generation_match: Generation,
    ) -> anyhow::Result<Stored> {
        if generation_of(&self.read().await?) != if_generation_match {
            return Ok(Stored::Conflict);
        }
        let mut temporary = OsString::from(self.path.as_os_str());
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        tokio::fs::write(&temporary, apps.to_json()?)
            .await
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        tokio::fs::rename(&temporary, &self.path)
            .await
            .with_context(|| format!("failed to replace {}", self.path.display()))?;
        Ok(Stored::Written)
    }

    async fn read(&self) -> anyhow::Result<Vec<u8>> {
        tokio::fs::read(&self.path)
            .await
            .with_context(|| format!("failed to read the registry file {}", self.path.display()))
    }
}

fn generation_of(bytes: &[u8]) -> Generation {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Generation(hasher.finish())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{test_support::app, testing::TemporaryRegistryFile};

    #[tokio::test]
    async fn load_reads_the_apps_written_to_the_file() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        file.put(&json!([{ "appId": "a", "secret": "s", "isRelay": false }]));

        // Act
        let snapshot = file.store().load().await.unwrap();

        // Assert
        assert_eq!(snapshot.apps.get("a").unwrap().secret, "s");
    }

    #[tokio::test]
    async fn load_reports_another_generation_after_the_contents_change() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        file.put(&json!([]));
        let store = file.store();
        let before = store.load().await.unwrap().generation;

        // Act
        file.put(&json!([{ "appId": "a", "secret": "s", "isRelay": false }]));

        // Assert
        assert_ne!(store.load().await.unwrap().generation, before);
    }

    #[tokio::test]
    async fn update_runs_the_change_again_when_the_file_changed_in_between() {
        // Arrange
        let file = TemporaryRegistryFile::new();
        file.put(&json!([]));
        let mut attempts = 0;

        // Act
        file.store()
            .update(|apps| {
                attempts += 1;
                if attempts == 1 {
                    file.put(&json!([{ "appId": "other", "secret": "s", "isRelay": false }]));
                }
                apps.push(app("a", Some("sub-1")));
                Ok(())
            })
            .await
            .unwrap();

        // Assert
        assert_eq!(attempts, 2);
        assert_eq!(
            file.body(),
            json!([
                { "appId": "other", "secret": "s", "isRelay": false },
                { "appId": "a", "secret": "a-secret", "isRelay": false, "sub": "sub-1" },
            ])
        );
    }

    #[tokio::test]
    async fn load_fails_when_the_file_does_not_exist() {
        // Arrange
        let file = TemporaryRegistryFile::new();

        // Act / Assert
        assert!(file.store().load().await.is_err());
    }
}
