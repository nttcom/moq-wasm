use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use moqt::object::{ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus};
use object_store::{ObjectStore, ObjectStoreExt, path::Path};
use serde_json::{Value, json};

use crate::{
    archive_chunks::{MAX_CHUNK_BYTES, digest},
    store::RecordingStore,
};

const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_TRACKS: usize = 256;

pub(crate) struct StoredTrack {
    pub(crate) namespace: String,
    pub(crate) source_namespace: String,
    pub(crate) content: String,
    pub(crate) name: String,
    pub(crate) chunks: Vec<Value>,
    pub(crate) first: Location,
    pub(crate) last: Location,
}

pub(crate) struct StoredRecording {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
    pub(crate) tracks: Vec<Arc<StoredTrack>>,
}

pub(crate) struct StoredEntry {
    pub(crate) chunk: usize,
    pub(crate) value: Value,
    pub(crate) location: Location,
}

impl StoredRecording {
    pub(crate) async fn open(store: &RecordingStore, id: &str, room: &str) -> Result<Self> {
        let mut recording = Self {
            store: store.object_store(),
            prefix: store.recording_prefix(id),
            tracks: Vec::new(),
        };
        let mut manifest: Value = serde_json::from_slice(
            &recording
                .read(
                    store.recording_prefix(id).join("manifest.json").as_ref(),
                    MAX_MANIFEST_BYTES,
                )
                .await?,
        )?;
        if manifest["format"] != "moq" {
            manifest = crate::mp4_replay::import(store, id, room, &manifest).await?;
        }
        ensure!(
            manifest["version"] == 1 && manifest["format"] == "moq",
            "unsupported recording format"
        );
        let members = manifest["members"]
            .as_array()
            .context("recording has no members")?;
        for (member_index, member) in members.iter().enumerate() {
            let source_namespace = string(member, "namespace")?;
            ensure!(
                source_namespace.starts_with(&format!("{room}/")),
                "member is outside the recorded room"
            );
            for content in ["camera", "screen_share"] {
                let sources = member[content]
                    .as_array()
                    .context("member has no archive list")?;
                for (source_index, source) in sources.iter().enumerate() {
                    let key = source.as_str().context("archive key is not a string")?;
                    let bytes = recording.read(key, MAX_MANIFEST_BYTES).await?;
                    let archive: Value = serde_json::from_slice(&bytes)?;
                    ensure!(
                        archive["version"] == 1 && archive["namespace"] == source_namespace,
                        "invalid broadcast archive"
                    );
                    for track in archive["tracks"]
                        .as_array()
                        .context("archive has no tracks")?
                    {
                        ensure!(
                            recording.tracks.len() < MAX_TRACKS,
                            "recording has too many tracks"
                        );
                        let chunks = track["chunks"]
                            .as_array()
                            .context("track has no chunks")?
                            .clone();
                        ensure!(chunks.len() <= 100_000, "track has too many chunks");
                        let mut locations = Vec::new();
                        for chunk in &chunks {
                            if chunk["first"].is_null() && chunk["last"].is_null() {
                                ensure!(
                                    integer(chunk, "data_size")? == 0,
                                    "payload chunk has no object locations"
                                );
                                continue;
                            }
                            let first = location(&chunk["first"])?;
                            let last = location(&chunk["last"])?;
                            ensure!(first <= last, "chunk has an invalid object range");
                            locations.push((first, last));
                        }
                        let Some(first) = locations.iter().map(|pair| pair.0).min() else {
                            continue;
                        };
                        let last = locations
                            .iter()
                            .map(|pair| pair.1)
                            .max()
                            .context("track has no last object")?;
                        ensure!(first <= last, "track has an invalid object range");
                        recording.tracks.push(Arc::new(StoredTrack {
                            namespace: format!(
                                "{room}/recordings/{id}/{member_index}/{content}/{source_index}"
                            ),
                            source_namespace: source_namespace.to_string(),
                            content: content.to_string(),
                            name: string(track, "name")?.to_string(),
                            chunks,
                            first,
                            last,
                        }));
                    }
                }
            }
        }
        ensure!(
            !recording.tracks.is_empty(),
            "recording contains no objects"
        );
        Ok(recording)
    }

    pub(crate) fn description(&self) -> Value {
        json!({
            "tracks": self.tracks.iter().map(|track| json!({
                "namespace": track.namespace, "source_namespace": track.source_namespace,
                "content": track.content, "name": track.name,
                "first": location_json(track.first), "last": location_json(track.last),
                "chunks": track.chunks.iter().map(|chunk| json!({
                    "first": chunk["first"], "last": chunk["last"],
                    "capture_start_us": chunk["capture_start_us"],
                    "capture_end_us": chunk["capture_end_us"],
                    "data_size": chunk["data_size"],
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "clock": { "source": "publisher_unix", "alignment": "approximate", "uncertainty_us": null },
        })
    }

    #[cfg(test)]
    pub(crate) async fn objects(&self, chunk: &Value) -> Result<Vec<Object>> {
        let data = self.checked_bytes(chunk, "data").await?;
        let index = self.checked_bytes(chunk, "index").await?;
        let entries: Vec<Value> = serde_json::from_slice(&index)?;
        ensure!(
            entries.len() <= 4096,
            "archive chunk contains too many entries"
        );
        entries
            .iter()
            .filter(|entry| entry["type"] == "object")
            .map(|entry| decode_object(entry, &data))
            .collect()
    }

    pub(crate) async fn entries(
        &self,
        track: &StoredTrack,
        start: Location,
        end: Location,
    ) -> Result<Vec<StoredEntry>> {
        let mut entries = Vec::new();
        let mut index_bytes = 0usize;
        for (chunk_index, chunk) in track.chunks.iter().enumerate() {
            let Some(first) = chunk["first"].as_object() else {
                continue;
            };
            let first = location(&Value::Object(first.clone()))?;
            let last = location(&chunk["last"])?;
            if last < start
                || (first >= end && !(end.object_id == 0 && first.group_id == end.group_id))
            {
                continue;
            }
            let index = self.checked_bytes(chunk, "index").await?;
            index_bytes = index_bytes
                .checked_add(index.len())
                .context("archive index size overflow")?;
            ensure!(
                index_bytes <= 32 * 1024 * 1024,
                "FETCH range indexes exceed 32 MiB; request a smaller range"
            );
            let values: Vec<Value> = serde_json::from_slice(&index)?;
            ensure!(
                values.len() <= 4096,
                "archive chunk contains too many entries"
            );
            for value in values.into_iter().filter(|entry| entry["type"] == "object") {
                let at = location(&value)?;
                ensure!(
                    at >= first && at <= last,
                    "object is outside its indexed location range"
                );
                if at >= start && (at < end || (at.group_id == end.group_id && end.object_id == 0))
                {
                    entries.push(StoredEntry {
                        chunk: chunk_index,
                        value,
                        location: at,
                    });
                }
            }
        }
        entries.sort_by_key(|entry| entry.location);
        Ok(entries)
    }

    pub(crate) async fn data(&self, chunk: &Value) -> Result<Bytes> {
        self.checked_bytes(chunk, "data").await
    }

    async fn checked_bytes(&self, chunk: &Value, field: &str) -> Result<Bytes> {
        let size = integer(chunk, &format!("{field}_size"))?;
        ensure!(size <= MAX_CHUNK_BYTES as u64, "archive chunk is too large");
        let bytes = self.read(string(chunk, field)?, MAX_CHUNK_BYTES).await?;
        ensure!(
            bytes.len() as u64 == size
                && digest(&bytes) == string(chunk, &format!("{field}_sha256"))?,
            "archive chunk checksum or size mismatch"
        );
        Ok(bytes)
    }

    async fn read(&self, key: &str, limit: usize) -> Result<Bytes> {
        ensure!(
            key.starts_with(&format!("{}/", self.prefix)),
            "archive key is outside the recording"
        );
        let result = self.store.get(&Path::parse(key)?).await?;
        ensure!(
            result.meta.size <= limit as u64,
            "archive object is too large"
        );
        let bytes = result.bytes().await?;
        ensure!(bytes.len() <= limit, "archive object is too large");
        Ok(bytes)
    }
}

pub(crate) fn decode_object(entry: &Value, data: &Bytes) -> Result<Object> {
    let offset = usize::try_from(integer(entry, "offset")?)?;
    let length = usize::try_from(integer(entry, "length")?)?;
    let end = offset
        .checked_add(length)
        .context("archive payload range overflow")?;
    ensure!(end <= data.len(), "archive payload is outside the chunk");
    let status = ObjectStatus::try_from(u8::try_from(
        entry["status"].as_u64().context("missing object status")?,
    )?)
    .map_err(|_| anyhow::anyhow!("invalid object status"))?;
    ensure!(
        length == 0 || status == ObjectStatus::Normal,
        "nonempty object has a non-normal status"
    );
    let extensions = STANDARD.decode(string(entry, "extensions")?)?;
    let mut cursor = extensions.as_slice();
    let extension_headers =
        ExtensionHeaders::decode(&mut cursor).context("invalid extension headers")?;
    ensure!(cursor.is_empty(), "trailing extension header data");
    let subgroup = match &entry["subgroup"] {
        Value::Null => ForwardingPreference::Datagram,
        _ => ForwardingPreference::Subgroup {
            subgroup_id: integer(entry, "subgroup")?,
        },
    };
    Ok(Object {
        location: location(entry)?,
        forwarding: subgroup,
        publisher_priority: u8::try_from(
            entry["priority"]
                .as_u64()
                .context("missing object priority")?,
        )?,
        status,
        extension_headers,
        payload: data.slice(offset..end),
        ends_group: entry["ends_group"]
            .as_bool()
            .context("missing ends_group")?,
    })
}

pub(crate) fn integer(value: &Value, key: &str) -> Result<u64> {
    string(value, key)?
        .parse()
        .with_context(|| format!("invalid {key}"))
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("missing {key}"))
}

pub(crate) fn location(value: &Value) -> Result<Location> {
    let group_id = integer(value, "group")?;
    let object_id = integer(value, "object")?;
    ensure!(
        group_id < (1 << 62) && object_id < (1 << 62) - 1,
        "object location exceeds MoQ varint range"
    );
    Ok(Location {
        group_id,
        object_id,
    })
}

fn location_json(location: Location) -> Value {
    json!({ "group": location.group_id.to_string(), "object": location.object_id.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{archive_chunks::ChunkWriter, archive_receiver::Event};
    use object_store::{PutPayload, memory::InMemory};

    #[tokio::test]
    async fn restores_raw_objects_and_rejects_corrupt_chunks_and_escaping_keys() {
        // Arrange
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let prefix = Path::from("id");
        let mut writer = ChunkWriter::new(
            store.clone(),
            prefix.clone().join("archive"),
            "anon/room/alice".into(),
        );
        let original = Object {
            location: Location {
                group_id: 23,
                object_id: 4,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 17,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::from_immutable_extensions(vec![
                Bytes::from_static(b"unknown"),
            ]),
            payload: Bytes::from_static(b"payload"),
            ends_group: true,
        };
        writer
            .push("video", &Event::Object(original.clone()))
            .await
            .unwrap();
        let key = writer.finish().await.unwrap();
        let manifest: Value =
            serde_json::from_slice(&store.get(&key).await.unwrap().bytes().await.unwrap()).unwrap();
        let chunk = manifest["tracks"][0]["chunks"][0].clone();
        let reader = StoredRecording {
            store: store.clone(),
            prefix,
            tracks: Vec::new(),
        };

        // Act / Assert
        assert_eq!(reader.objects(&chunk).await.unwrap(), [original]);
        store
            .put(
                &Path::from(chunk["data"].as_str().unwrap()),
                PutPayload::from("corrupt"),
            )
            .await
            .unwrap();
        assert!(
            reader
                .objects(&chunk)
                .await
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        assert!(reader.read("another-id/secret", 100).await.is_err());
        let mut entry = json!({ "offset": u64::MAX.to_string(), "length": "2" });
        assert!(decode_object(&entry, &Bytes::new()).is_err());
        entry["offset"] = json!("0");
        assert!(decode_object(&entry, &Bytes::new()).is_err());
    }
}
