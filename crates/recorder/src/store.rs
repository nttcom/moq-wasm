use std::sync::Arc;

use anyhow::{Context, Result};
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutPayload, path::Path};
use serde_json::Value;
use url::Url;

use crate::{
    manifest::MANIFEST_NAME,
    room_state::{CompositeState, RoomRecord},
};

/// The store a recording service keeps its recordings in, and the objects
/// through which the API and the jobs share their state. Every object is
/// written by one side at a time, so a plain overwrite never races and no
/// store needs compare-and-swap:
///
/// - `state/<id>.json`: the `RoomRecord`, written by the API once and then by
///   the recording job.
/// - `state/<id>.stop`: the API's request to stop.
/// - `state/<id>.composite.json`: the `CompositeState`, written by the
///   composite job, or by the recording job when it cannot start one.
/// - `locks/<namespace prefix>`: the id recording the room, created by the
///   API only if absent, deleted by the recording job when it ends, or by
///   the API once that recording is no longer active or its job failed to
///   launch.
/// - `rooms/<namespace prefix>/<id>`: every recording of a room, so that a
///   room's recordings are listed without reading the others.
/// - `<id>/...`: the recording's files and its manifest.
///
/// A namespace prefix is one path element (its `/` percent-encoded), so the
/// room `anon/a` and the room `anon/a/b` never nest in a directory store.
#[derive(Clone)]
pub(crate) struct RecordingStore {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
}

impl RecordingStore {
    /// `gs://<bucket>/<prefix>` or `file:///<directory>`; credentials come
    /// from the environment.
    pub(crate) fn open(url: &Url) -> Result<Self> {
        let (store, prefix) = object_store::parse_url_opts(url, std::env::vars())
            .with_context(|| format!("cannot use {url} as the recording store"))?;
        Ok(Self {
            store: Arc::from(store),
            prefix,
        })
    }

    pub(crate) fn object_store(&self) -> Arc<dyn ObjectStore> {
        Arc::clone(&self.store)
    }

    /// Where a recording's files go.
    pub(crate) fn recording_prefix(&self, id: &str) -> Path {
        self.prefix.clone().join(id)
    }

    pub(crate) async fn write_state(&self, record: &RoomRecord) -> Result<()> {
        self.put_json(&self.state_key(&record.id, "json"), &record.to_json())
            .await
    }

    pub(crate) async fn read_state(&self, id: &str) -> Result<Option<RoomRecord>> {
        self.get_json(&self.state_key(id, "json"))
            .await?
            .map(|value| RoomRecord::from_json(&value))
            .transpose()
            .with_context(|| format!("malformed state of recording {id}"))
    }

    pub(crate) async fn write_composite(&self, id: &str, state: &CompositeState) -> Result<()> {
        self.put_json(&self.state_key(id, "composite.json"), &state.to_json())
            .await
    }

    pub(crate) async fn read_manifest(&self, id: &str) -> Result<Option<Value>> {
        self.get_json(&self.recording_prefix(id).join(MANIFEST_NAME))
            .await
    }

    pub(crate) async fn read_composite(&self, id: &str) -> Result<Option<CompositeState>> {
        self.get_json(&self.state_key(id, "composite.json"))
            .await?
            .map(|value| CompositeState::from_json(&value))
            .transpose()
            .with_context(|| format!("malformed composite state of recording {id}"))
    }

    pub(crate) async fn request_stop(&self, id: &str) -> Result<()> {
        self.store
            .put(&self.state_key(id, "stop"), PutPayload::new())
            .await
            .with_context(|| format!("cannot request recording {id} to stop"))?;
        Ok(())
    }

    /// Takes the room for `id`; false when a recording holds it already.
    pub(crate) async fn lock_room(&self, namespace_prefix: &str, id: &str) -> Result<bool> {
        let key = self.lock_key(namespace_prefix);
        match self
            .store
            .put_opts(
                &key,
                PutPayload::from(id.to_string()),
                PutMode::Create.into(),
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. }) => Ok(false),
            Err(error) => Err(error).with_context(|| format!("cannot write {key}")),
        }
    }

    /// The id of the recording holding the room.
    pub(crate) async fn room_lock(&self, namespace_prefix: &str) -> Result<Option<String>> {
        Ok(self
            .get(&self.lock_key(namespace_prefix))
            .await?
            .map(|id| String::from_utf8_lossy(&id).into_owned()))
    }

    /// The ids of the recordings holding a room.
    pub(crate) async fn lock_holders(&self) -> Result<Vec<String>> {
        let locks = self.prefix.clone().join("locks");
        let mut holders = Vec::new();
        for lock in self.store.list_with_delimiter(Some(&locks)).await?.objects {
            if let Some(id) = self.get(&lock.location).await? {
                holders.push(String::from_utf8_lossy(&id).into_owned());
            }
        }
        Ok(holders)
    }

    pub(crate) async fn add_to_room(&self, namespace_prefix: &str, id: &str) -> Result<()> {
        let key = self.room_key(namespace_prefix).join(id);
        self.store
            .put(&key, PutPayload::new())
            .await
            .with_context(|| format!("cannot write {key}"))?;
        Ok(())
    }

    pub(crate) async fn room_recording_ids(&self, namespace_prefix: &str) -> Result<Vec<String>> {
        let listed = self
            .store
            .list_with_delimiter(Some(&self.room_key(namespace_prefix)))
            .await?;
        Ok(listed
            .objects
            .iter()
            .filter_map(|object| object.location.filename().map(str::to_string))
            .collect())
    }

    pub(crate) async fn recording_ids(&self) -> Result<Vec<String>> {
        let state = self.prefix.clone().join("state");
        let listed = self.store.list_with_delimiter(Some(&state)).await?;
        Ok(listed
            .objects
            .iter()
            .filter_map(|object| object.location.filename())
            .filter(|name| !name.ends_with(".composite.json"))
            .filter_map(|name| name.strip_suffix(".json"))
            .map(str::to_string)
            .collect())
    }

    pub(crate) async fn stop_requested(&self, id: &str) -> Result<bool> {
        Ok(self.get(&self.state_key(id, "stop")).await?.is_some())
    }

    /// Deletes the room's lock if `id` holds it.
    pub(crate) async fn unlock_room(&self, namespace_prefix: &str, id: &str) -> Result<()> {
        let key = self.lock_key(namespace_prefix);
        if self.get(&key).await?.as_deref() == Some(id.as_bytes()) {
            delete_if_present(&self.store, &key).await?;
        }
        Ok(())
    }

    async fn put_json(&self, key: &Path, value: &Value) -> Result<()> {
        self.store
            .put(key, PutPayload::from(value.to_string()))
            .await
            .with_context(|| format!("cannot write {key}"))?;
        Ok(())
    }

    async fn get_json(&self, key: &Path) -> Result<Option<Value>> {
        self.get(key)
            .await?
            .map(|bytes| serde_json::from_slice(&bytes).with_context(|| format!("{key}")))
            .transpose()
    }

    async fn get(&self, key: &Path) -> Result<Option<bytes::Bytes>> {
        match self.store.get(key).await {
            Ok(object) => Ok(Some(object.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error).with_context(|| format!("cannot read {key}")),
        }
    }

    fn state_key(&self, id: &str, extension: &str) -> Path {
        self.prefix
            .clone()
            .join("state")
            .join(format!("{id}.{extension}"))
    }

    fn lock_key(&self, namespace_prefix: &str) -> Path {
        self.prefix.clone().join("locks").join(namespace_prefix)
    }

    fn room_key(&self, namespace_prefix: &str) -> Path {
        self.prefix.clone().join("rooms").join(namespace_prefix)
    }
}

async fn delete_if_present(store: &Arc<dyn ObjectStore>, key: &Path) -> Result<()> {
    match store.delete(key).await {
        Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot delete {key}")),
    }
}

#[cfg(test)]
mod tests {
    use object_store::local::LocalFileSystem;

    use super::*;
    use crate::{
        recording::Layout,
        room_state::{MemberStatus, RoomState},
        test_support::scratch_dir,
    };

    fn record() -> RoomRecord {
        RoomRecord {
            id: "1700000000000-a1b2c3".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::Combined,
            composite: false,
            state: RoomState::Recording,
            members: vec![MemberStatus {
                namespace: "anon/research/ucchy".to_string(),
                started_at_ms: 1_700_000_000_100,
                finished: false,
                error: None,
            }],
            updated_at_ms: 1_700_000_000_000,
        }
    }

    #[tokio::test]
    async fn keeps_the_state_in_a_directory() {
        // Arrange
        let dir = scratch_dir();
        let url = Url::from_directory_path(&dir).unwrap();
        let store = RecordingStore::open(&url).unwrap();

        // Act
        store.write_state(&record()).await.unwrap();
        let read = store.read_state(&record().id).await.unwrap();
        let missing = store.read_state("1700000000000-ffffff").await.unwrap();
        let stop = store.stop_requested(&record().id).await.unwrap();

        // Assert
        let written = dir.join("state/1700000000000-a1b2c3.json").exists();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(written);
        assert_eq!(read, Some(record()));
        assert_eq!(missing, None);
        assert!(!stop);
    }

    #[tokio::test]
    async fn unlocks_a_room_only_for_the_recording_holding_it() {
        // Arrange
        let dir = scratch_dir();
        let store = RecordingStore {
            store: Arc::new(LocalFileSystem::new_with_prefix(&dir).unwrap()),
            prefix: Path::default(),
        };
        let key = store.lock_key("anon/research");
        store.store.put(&key, "newer".into()).await.unwrap();

        // Act
        store.unlock_room("anon/research", "older").await.unwrap();
        let kept = store.get(&key).await.unwrap().is_some();
        store.unlock_room("anon/research", "newer").await.unwrap();

        // Assert
        let deleted = store.get(&key).await.unwrap().is_none();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(kept);
        assert!(deleted);
    }
}
