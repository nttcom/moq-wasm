use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use object_store::{
    ObjectMeta, ObjectStore, ObjectStoreExt, PutMode, PutPayload,
    gcp::GoogleCloudStorageBuilder,
    path::Path,
    signer::{Method, Signer},
};
use serde_json::Value;
use url::Url;

use crate::{
    cloud_storage::CloudStorageBucket,
    manifest::MANIFEST_NAME,
    room_state::{CompositeState, RoomRecord},
};

/// The store a recording service keeps its recordings in, and the objects
/// through which the API and the jobs share their state. Every object but a
/// lock is written by one side at a time, so a plain overwrite never races:
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
/// - `<app>/<id>/...`: the recording's files and its manifest, under the app
///   that owns the room, the first element of its namespace prefix, so that
///   an app's recordings are listed without reading the others.
///
/// A namespace prefix is one path element (its `/` percent-encoded), so the
/// room `anon/a` and the room `anon/a/b` never nest in a directory store.
#[derive(Clone)]
pub(crate) struct RecordingStore {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
    cloud_storage: Option<Arc<CloudStorageBucket>>,
    /// Signs the URLs a browser reads a file with; only a `gs://` store has one.
    signer: Option<Arc<dyn Signer>>,
}

/// `name` is the file's key below the recording's prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredFile {
    pub(crate) name: String,
    pub(crate) size_bytes: u64,
}

impl RecordingStore {
    /// `gs://<bucket>/<prefix>` or `file:///<directory>`; credentials come
    /// from the environment.
    pub(crate) fn open(url: &Url) -> Result<Self> {
        let (store, prefix) = object_store::parse_url_opts(url, std::env::vars())
            .with_context(|| format!("cannot use {url} as the recording store"))?;
        let (cloud_storage, signer) = match (url.scheme(), url.host_str()) {
            ("gs", Some(bucket)) => {
                let signer: Arc<dyn Signer> = Arc::new(
                    GoogleCloudStorageBuilder::from_env()
                        .with_bucket_name(bucket)
                        .build()?,
                );
                (
                    Some(Arc::new(CloudStorageBucket::new(bucket)?)),
                    Some(signer),
                )
            }
            _ => (None, None),
        };
        Ok(Self {
            store: Arc::from(store),
            prefix,
            cloud_storage,
            signer,
        })
    }

    pub(crate) fn object_store(&self) -> Arc<dyn ObjectStore> {
        Arc::clone(&self.store)
    }

    /// Where the files of recording `id` of the room `namespace_prefix` go.
    pub(crate) fn recording_prefix(&self, namespace_prefix: &str, id: &str) -> Path {
        let app = namespace_prefix.split('/').next().unwrap_or_default();
        self.prefix.clone().join(app).join(id)
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

    pub(crate) async fn read_manifest(
        &self,
        namespace_prefix: &str,
        id: &str,
    ) -> Result<Option<Value>> {
        self.get_json(
            &self
                .recording_prefix(namespace_prefix, id)
                .join(MANIFEST_NAME),
        )
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

    /// The ids of the recordings of `app` that have a file in the store,
    /// newest first.
    pub(crate) async fn app_recording_ids(&self, app: &str) -> Result<Vec<String>> {
        let listed = self
            .store
            .list_with_delimiter(Some(&self.prefix.clone().join(app)))
            .await?;
        let mut ids: Vec<String> = listed
            .common_prefixes
            .iter()
            .filter_map(|prefix| prefix.filename().map(str::to_string))
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        Ok(ids)
    }

    /// The MP4 files of a recording, sorted by name.
    pub(crate) async fn recording_files(
        &self,
        namespace_prefix: &str,
        id: &str,
    ) -> Result<Vec<StoredFile>> {
        let prefix = self.recording_prefix(namespace_prefix, id);
        let mut files: Vec<StoredFile> = self
            .list_recursively(&prefix)
            .await?
            .into_iter()
            .filter(|object| object.location.extension() == Some("mp4"))
            .filter_map(|object| {
                let name = object
                    .location
                    .prefix_match(&prefix)?
                    .map(|part| part.as_ref().to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                Some(StoredFile {
                    name,
                    size_bytes: object.size,
                })
            })
            .collect();
        files.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    /// A URL that reads the MP4 file `name` of a recording for `expires_in`;
    /// `None` when the recording has no such file. Fails on a store that
    /// cannot sign (any but `gs://`).
    pub(crate) async fn file_url(
        &self,
        namespace_prefix: &str,
        id: &str,
        name: &str,
        expires_in: Duration,
    ) -> Result<Option<url::Url>> {
        let prefix = self.recording_prefix(namespace_prefix, id);
        let Ok(key) = Path::parse(format!("{prefix}/{name}")) else {
            return Ok(None);
        };
        if !key.prefix_matches(&prefix) || key.extension() != Some("mp4") {
            return Ok(None);
        }
        match self.store.head(&key).await {
            Ok(_) => {}
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("cannot read {key}")),
        }
        let signer = self.signer.as_ref().context("the store cannot sign URLs")?;
        Ok(Some(
            signer.signed_url(Method::GET, &key, expires_in).await?,
        ))
    }

    async fn list_recursively(&self, prefix: &Path) -> Result<Vec<ObjectMeta>> {
        let mut pending = vec![prefix.clone()];
        let mut objects = Vec::new();
        while let Some(directory) = pending.pop() {
            let listed = self.store.list_with_delimiter(Some(&directory)).await?;
            objects.extend(listed.objects);
            pending.extend(listed.common_prefixes);
        }
        Ok(objects)
    }

    pub(crate) async fn stop_requested(&self, id: &str) -> Result<bool> {
        Ok(self.get(&self.state_key(id, "stop")).await?.is_some())
    }

    /// Deletes the room's lock if `id` holds it. In Cloud Storage a lock
    /// another request took after it was read stays.
    pub(crate) async fn unlock_room(&self, namespace_prefix: &str, id: &str) -> Result<()> {
        let key = self.lock_key(namespace_prefix);
        let lock = match self.store.get(&key).await {
            Ok(lock) => lock,
            Err(object_store::Error::NotFound { .. }) => return Ok(()),
            Err(error) => return Err(error).with_context(|| format!("cannot read {key}")),
        };
        let generation = lock.meta.version.clone();
        if lock.bytes().await? != id.as_bytes() {
            return Ok(());
        }
        let (Some(bucket), Some(generation)) = (&self.cloud_storage, generation) else {
            return delete_if_present(&self.store, &key).await;
        };
        bucket
            .delete_if_generation_matches(key.as_ref(), &generation)
            .await
            .with_context(|| format!("cannot delete {key}"))
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
        room_state::{MemberStatus, RecordingFormat, RoomState},
        test_support::scratch_dir,
    };

    fn record() -> RoomRecord {
        RoomRecord {
            id: "1700000000000-a1b2c3".to_string(),
            namespace_prefix: "anon/research".to_string(),
            layout: Layout::Combined,
            format: RecordingFormat::Mp4,
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
            cloud_storage: None,
            signer: None,
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

    #[tokio::test]
    async fn lists_an_apps_recordings_newest_first_with_their_mp4_files() {
        // Arrange
        let store = RecordingStore::open(&Url::parse("memory:///").unwrap()).unwrap();
        let put = |key: Path, bytes: &'static [u8]| {
            let store = store.object_store();
            async move { store.put(&key, PutPayload::from(bytes)).await.unwrap() }
        };
        let older = store.recording_prefix("app/room", "1700000000000-000001");
        let newer = store.recording_prefix("app/room", "1700000001000-000002");
        put(
            older
                .clone()
                .join("app")
                .join("room")
                .join("alice")
                .join("10-1.mp4"),
            b"12345",
        )
        .await;
        put(older.clone().join("manifest.json"), b"{}").await;
        put(newer.clone().join("composite.mp4"), b"1").await;
        put(
            store
                .recording_prefix("other/room", "1700000002000-000003")
                .join("a.mp4"),
            b"1",
        )
        .await;

        // Act
        let ids = store.app_recording_ids("app").await.unwrap();
        let files = store
            .recording_files("app/room", "1700000000000-000001")
            .await
            .unwrap();

        // Assert
        assert_eq!(ids, ["1700000001000-000002", "1700000000000-000001"]);
        assert_eq!(
            files,
            [StoredFile {
                name: "app/room/alice/10-1.mp4".to_string(),
                size_bytes: 5,
            }]
        );
    }

    #[tokio::test]
    async fn signs_only_an_mp4_file_of_the_recording() {
        // Arrange: a memory store, which cannot sign
        let store = RecordingStore::open(&Url::parse("memory:///").unwrap()).unwrap();
        let prefix = store.recording_prefix("app/room", "1");
        for name in ["a.mp4", "manifest.json"] {
            store
                .object_store()
                .put(&prefix.clone().join(name), PutPayload::from_static(b"1"))
                .await
                .unwrap();
        }
        store
            .object_store()
            .put(
                &store.recording_prefix("app/room", "2").join("b.mp4"),
                PutPayload::from_static(b"1"),
            )
            .await
            .unwrap();
        let url = |name: &'static str| {
            let store = store.clone();
            async move {
                store
                    .file_url("app/room", "1", name, Duration::from_secs(60))
                    .await
            }
        };

        // Act / Assert
        assert!(url("missing.mp4").await.unwrap().is_none());
        assert!(url("manifest.json").await.unwrap().is_none());
        assert!(url("../2/b.mp4").await.unwrap().is_none());
        assert!(url("a.mp4").await.is_err());
    }
}
