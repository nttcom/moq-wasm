use std::sync::Arc;

use anyhow::{Context, Result};
use object_store::{
    ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, UpdateVersion, path::Path,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BotState {
    Starting,
    Running,
    Stopping,
    Finished,
    Failed,
}

impl BotState {
    pub(crate) fn has_ended(self) -> bool {
        matches!(self, Self::Finished | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BotRecord {
    pub(crate) id: String,
    pub(crate) bot_type: String,
    pub(crate) app_id: String,
    pub(crate) namespace_prefix: String,
    pub(crate) state: BotState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    /// SHA-256 of the key a locally launched bot authenticates with, in hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) key_sha256: Option<String>,
}

/// The room's current holder for one bot type.
pub(crate) struct RoomHolder {
    pub(crate) id: String,
    version: UpdateVersion,
}

/// The objects `bot-dispatcher` keeps its state in, so that any number of instances
/// answer alike:
///
/// - `bots/<id>.json`: the `BotRecord`, written from the launch request and
///   then from the bot's heartbeats.
/// - `bots/<id>.stop`: a request to stop the bot.
/// - `rooms/<type>/<namespace prefix>`: the id of the room's latest bot of
///   that type. It is created only if absent and replaced only at the version
///   that was read, so one room runs at most one bot of each type; it is
///   never deleted. A namespace prefix is one path element (its `/`
///   percent-encoded).
#[derive(Clone)]
pub(crate) struct BotStore {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
}

impl BotStore {
    /// `gs://<bucket>/<prefix>` or `file:///<directory>`; credentials come
    /// from the environment.
    pub(crate) fn open(url: &Url) -> Result<Self> {
        let (store, prefix) = object_store::parse_url_opts(url, std::env::vars())
            .with_context(|| format!("cannot use {url} as the bot store"))?;
        Ok(Self {
            store: Arc::from(store),
            prefix,
        })
    }

    pub(crate) async fn write(&self, record: &BotRecord) -> Result<()> {
        let key = self.bot_key(&record.id, "json");
        self.store
            .put(&key, PutPayload::from(serde_json::to_vec(record)?))
            .await
            .with_context(|| format!("cannot write {key}"))?;
        Ok(())
    }

    pub(crate) async fn read(&self, id: &str) -> Result<Option<BotRecord>> {
        let key = self.bot_key(id, "json");
        self.get(&key)
            .await?
            .map(|bytes| serde_json::from_slice(&bytes).with_context(|| format!("{key}")))
            .transpose()
    }

    pub(crate) async fn request_stop(&self, id: &str) -> Result<()> {
        let key = self.bot_key(id, "stop");
        self.store
            .put(&key, PutPayload::default())
            .await
            .with_context(|| format!("cannot write {key}"))?;
        Ok(())
    }

    pub(crate) async fn stop_requested(&self, id: &str) -> Result<bool> {
        Ok(self.get(&self.bot_key(id, "stop")).await?.is_some())
    }

    pub(crate) async fn room_holder(
        &self,
        bot_type: &str,
        namespace_prefix: &str,
    ) -> Result<Option<RoomHolder>> {
        let key = self.room_key(bot_type, namespace_prefix);
        match self.store.get(&key).await {
            Ok(object) => {
                let version = UpdateVersion {
                    e_tag: object.meta.e_tag.clone(),
                    version: object.meta.version.clone(),
                };
                let id = String::from_utf8(object.bytes().await?.to_vec())
                    .with_context(|| format!("{key}"))?;
                Ok(Some(RoomHolder { id, version }))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error).with_context(|| format!("cannot read {key}")),
        }
    }

    /// Makes `id` the room's holder if the room has none (`replacing` is
    /// `None`) or still has the holder that was read. `false` when another
    /// request changed the holder first. A store without conditional updates
    /// (a `file://` directory) replaces the holder unconditionally.
    pub(crate) async fn take_room(
        &self,
        bot_type: &str,
        namespace_prefix: &str,
        id: &str,
        replacing: Option<RoomHolder>,
    ) -> Result<bool> {
        let key = self.room_key(bot_type, namespace_prefix);
        let mode = match replacing {
            None => PutMode::Create,
            Some(holder) => PutMode::Update(holder.version),
        };
        let payload = PutPayload::from(id.to_string());
        let result = self
            .store
            .put_opts(&key, payload.clone(), PutOptions::from(mode))
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(object_store::Error::AlreadyExists { .. })
            | Err(object_store::Error::Precondition { .. }) => Ok(false),
            Err(object_store::Error::NotImplemented { .. }) => {
                self.store
                    .put(&key, payload)
                    .await
                    .with_context(|| format!("cannot write {key}"))?;
                Ok(true)
            }
            Err(error) => Err(error).with_context(|| format!("cannot write {key}")),
        }
    }

    /// The ids holding a room for `bot_type`, in any app.
    pub(crate) async fn room_holders(&self, bot_type: &str) -> Result<Vec<String>> {
        let rooms = self.prefix.clone().join("rooms").join(bot_type);
        let objects = self
            .store
            .list_with_delimiter(Some(&rooms))
            .await
            .with_context(|| format!("cannot list {rooms}"))?
            .objects;
        let mut ids = Vec::with_capacity(objects.len());
        for object in objects {
            if let Some(bytes) = self.get(&object.location).await? {
                ids.push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        Ok(ids)
    }

    async fn get(&self, key: &Path) -> Result<Option<bytes::Bytes>> {
        match self.store.get(key).await {
            Ok(object) => Ok(Some(object.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error).with_context(|| format!("cannot read {key}")),
        }
    }

    fn bot_key(&self, id: &str, extension: &str) -> Path {
        self.prefix
            .clone()
            .join("bots")
            .join(format!("{id}.{extension}").as_str())
    }

    fn room_key(&self, bot_type: &str, namespace_prefix: &str) -> Path {
        self.prefix
            .clone()
            .join("rooms")
            .join(bot_type)
            .join(namespace_prefix)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::temporary_store;

    #[tokio::test]
    async fn a_room_is_taken_once_until_its_holder_is_replaced() {
        // Arrange
        let store = temporary_store();

        // Act
        let first = store
            .take_room("recorder", "app/room", "a", None)
            .await
            .unwrap();
        let second = store
            .take_room("recorder", "app/room", "b", None)
            .await
            .unwrap();
        let holder = store
            .room_holder("recorder", "app/room")
            .await
            .unwrap()
            .unwrap();
        let replaced = store
            .take_room("recorder", "app/room", "c", Some(holder))
            .await
            .unwrap();

        // Assert
        assert!(first && !second && replaced);
        let holder = store
            .room_holder("recorder", "app/room")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(holder.id, "c");
        assert_eq!(store.room_holders("recorder").await.unwrap(), ["c"]);
    }

    #[tokio::test]
    async fn rooms_with_slashes_do_not_nest() {
        // Arrange
        let store = temporary_store();

        // Act
        store
            .take_room("recorder", "app/a", "1", None)
            .await
            .unwrap();
        store
            .take_room("recorder", "app/a/b", "2", None)
            .await
            .unwrap();

        // Assert
        let mut holders = store.room_holders("recorder").await.unwrap();
        holders.sort();
        assert_eq!(holders, ["1", "2"]);
    }
}
