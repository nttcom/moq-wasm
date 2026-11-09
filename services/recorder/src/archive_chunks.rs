use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::loc::{self, LocObject};
use object_store::{ObjectStore, PutMode, PutPayload, path::Path};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::archive_receiver::Event;

const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const CHUNK_ENTRIES: usize = 4096;
pub(crate) const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;

struct Track {
    id: usize,
    data: Vec<u8>,
    entries: Vec<Value>,
    chunks: Vec<Value>,
    seen: HashMap<(u64, u64), [u8; 32]>,
}

/// Writes immutable data and index chunks before publishing their manifest.
pub(crate) struct ChunkWriter {
    store: Arc<dyn ObjectStore>,
    prefix: Path,
    namespace: String,
    tracks: HashMap<String, Track>,
}

impl ChunkWriter {
    pub(crate) fn new(store: Arc<dyn ObjectStore>, prefix: Path, namespace: String) -> Self {
        Self {
            store,
            prefix,
            namespace,
            tracks: HashMap::new(),
        }
    }

    /// An empty writer for archive `index` of the same broadcast: this one's
    /// prefix for 0, `<prefix>-<index>` after it.
    pub(crate) fn part(&self, index: usize) -> Self {
        let prefix = match index {
            0 => self.prefix.clone(),
            index => Path::from(format!("{}-{index}", self.prefix)),
        };
        Self::new(Arc::clone(&self.store), prefix, self.namespace.clone())
    }

    pub(crate) async fn push(&mut self, name: &str, event: &Event) -> Result<()> {
        let next_id = self.tracks.len();
        let track = self.tracks.entry(name.to_owned()).or_insert_with(|| Track {
            id: next_id,
            data: Vec::new(),
            entries: Vec::new(),
            chunks: Vec::new(),
            seen: HashMap::new(),
        });
        if let Event::Object(object) = event {
            let identity = (object.location.group_id, object.location.object_id);
            let fingerprint = object_fingerprint(object);
            if let Some(first) = track.seen.get(&identity) {
                anyhow::ensure!(
                    *first == fingerprint,
                    "conflicting object identity in track {name}"
                );
                return Ok(());
            }
            anyhow::ensure!(
                track.seen.len() < 1_000_000,
                "archive track exceeds one million objects"
            );
            track.seen.insert(identity, fingerprint);
        }
        let length = match event {
            Event::Object(object) => object.payload.len(),
            _ => 0,
        };
        anyhow::ensure!(length <= MAX_CHUNK_BYTES, "archive object exceeds 16 MiB");
        if !track.entries.is_empty()
            && (track.data.len().saturating_add(length) > CHUNK_BYTES
                || track.entries.len() >= CHUNK_ENTRIES)
        {
            flush(&self.store, &self.prefix, track).await?;
        }
        match event {
            Event::Header(header) => track.entries.push(json!({
                "type": "header", "group": header.group_id.to_string(),
                "subgroup": header.subgroup_id.to_string(), "priority": header.publisher_priority,
                "ends_group": header.ends_group,
            })),
            Event::End {
                group_id,
                subgroup_id,
            } => track.entries.push(json!({
                "type": "end", "group": group_id.to_string(), "subgroup": subgroup_id.to_string(),
            })),
            Event::Object(object) => {
                use base64::{Engine, engine::general_purpose::STANDARD};
                let subgroup = match object.forwarding {
                    moqt::object::ForwardingPreference::Subgroup { subgroup_id } => {
                        Some(subgroup_id.to_string())
                    }
                    moqt::object::ForwardingPreference::Datagram => None,
                };
                let offset = track.data.len();
                track.data.extend_from_slice(&object.payload);
                let timestamp = LocObject {
                    extensions: loc::from_extension_headers(&object.extension_headers),
                    payload: Bytes::new(),
                }
                .timestamp()
                .map(|timestamp| timestamp.micros().to_string());
                track.entries.push(json!({
                    "type": "object", "group": object.location.group_id.to_string(),
                    "subgroup": subgroup, "object": object.location.object_id.to_string(),
                    "priority": object.publisher_priority, "status": object.status as u8,
                    "ends_group": object.ends_group,
                    "timestamp_us": timestamp,
                    "extensions": STANDARD.encode(object.extension_headers.encode()),
                    "offset": offset.to_string(), "length": length.to_string(),
                }));
            }
        }
        Ok(())
    }

    pub(crate) async fn finish(mut self) -> Result<Path> {
        for track in self.tracks.values_mut() {
            if !track.entries.is_empty() {
                flush(&self.store, &self.prefix, track).await?;
            }
        }
        let mut tracks: Vec<_> = self.tracks.into_iter().collect();
        tracks.sort_by_key(|(_, track)| track.id);
        let tracks: Vec<_> = tracks
            .into_iter()
            .map(|(name, track)| {
                json!({
                    "id": track.id.to_string(), "name": name, "chunks": track.chunks,
                })
            })
            .collect();
        let manifest = serde_json::to_vec(
            &json!({ "version": 2, "namespace": self.namespace, "tracks": tracks }),
        )?;
        let key = self.prefix.clone().join("manifest.json");
        put_new(&self.store, &key, manifest).await?;
        Ok(key)
    }
}

async fn flush(store: &Arc<dyn ObjectStore>, prefix: &Path, track: &mut Track) -> Result<()> {
    let base = prefix.clone().join("tracks").join(track.id.to_string());
    let data_key = base.clone().join(format!("{:08}.data", track.chunks.len()));
    let index_key = base.join(format!("{:08}.index.json", track.chunks.len()));
    let data = std::mem::take(&mut track.data);
    let index = serde_json::to_vec(&track.entries)?;
    anyhow::ensure!(
        data.len() <= MAX_CHUNK_BYTES && index.len() <= MAX_CHUNK_BYTES,
        "archive chunk exceeds 16 MiB"
    );
    let data_digest = digest(&data);
    let index_digest = digest(&index);
    let data_len = data.len();
    let index_len = index.len();
    let locations = track.entries.iter().filter_map(|entry| {
        if entry["type"] != "object" {
            return None;
        }
        Some((
            entry["group"].as_str()?.parse::<u64>().ok()?,
            entry["object"].as_str()?.parse::<u64>().ok()?,
        ))
    });
    let first = locations.clone().min();
    let last = locations.max();
    // The groups with media, each with its first such object: a status object
    // alone carries none.
    let mut group_starts: BTreeMap<u64, (u64, Option<String>)> = BTreeMap::new();
    for entry in track.entries.iter().filter(|entry| {
        entry["type"] == "object" && entry["status"] == moqt::object::ObjectStatus::Normal as u8
    }) {
        let (Some(group), Some(object)) = (
            entry["group"].as_str().and_then(|group| group.parse().ok()),
            entry["object"]
                .as_str()
                .and_then(|object| object.parse().ok()),
        ) else {
            continue;
        };
        let start = group_starts.entry(group).or_insert((object, None));
        if object <= start.0 {
            *start = (object, entry["timestamp_us"].as_str().map(str::to_string));
        }
    }
    let timestamps = track
        .entries
        .iter()
        .filter_map(|entry| entry["timestamp_us"].as_str()?.parse::<u64>().ok());
    let timestamp_start = timestamps.clone().min();
    let timestamp_end = timestamps.max();
    put_new(store, &data_key, data).await?;
    put_new(store, &index_key, index).await?;
    track.entries.clear();
    track.chunks.push(json!({
        "data": data_key.as_ref(), "index": index_key.as_ref(),
        "data_size": data_len.to_string(), "index_size": index_len.to_string(),
        "data_sha256": data_digest, "index_sha256": index_digest,
        "first": first.map(|(group, object)| json!({ "group": group.to_string(), "object": object.to_string() })),
        "last": last.map(|(group, object)| json!({ "group": group.to_string(), "object": object.to_string() })),
        "timestamp_start_us": timestamp_start.map(|value| value.to_string()),
        "timestamp_end_us": timestamp_end.map(|value| value.to_string()),
        "groups": group_starts.iter().map(|(group, (object, timestamp))| json!({
            "group": group.to_string(), "object": object.to_string(), "timestamp_us": timestamp,
        })).collect::<Vec<_>>(),
    }));
    Ok(())
}

async fn put_new(store: &Arc<dyn ObjectStore>, key: &Path, data: Vec<u8>) -> Result<()> {
    store
        .put_opts(
            key,
            PutPayload::from(Bytes::from(data)),
            PutMode::Create.into(),
        )
        .await
        .with_context(|| format!("cannot write archive object {key}"))?;
    Ok(())
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn object_fingerprint(object: &moqt::object::Object) -> [u8; 32] {
    let mut hash = Sha256::new();
    match object.forwarding {
        moqt::object::ForwardingPreference::Datagram => hash.update([0]),
        moqt::object::ForwardingPreference::Subgroup { subgroup_id } => {
            hash.update([1]);
            hash.update(subgroup_id.to_be_bytes());
        }
    }
    hash.update([
        object.publisher_priority,
        object.status as u8,
        u8::from(object.ends_group),
    ]);
    hash.update(object.extension_headers.encode());
    hash.update(&object.payload);
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use moqt::object::{ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus};
    use object_store::{ObjectStoreExt, memory::InMemory};

    use super::*;

    #[tokio::test]
    async fn commits_data_and_index_before_manifest_without_overwriting() {
        // Arrange
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let prefix = Path::from("recording");
        let mut writer =
            ChunkWriter::new(store.clone(), prefix.clone(), "anon/room/alice".to_string());
        let object = Object {
            location: Location {
                group_id: u64::MAX,
                object_id: 4,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 50,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(b"payload"),
            ends_group: true,
        };

        // Act
        writer.push("video", &Event::Object(object)).await.unwrap();
        assert!(
            store
                .get(&prefix.clone().join("manifest.json"))
                .await
                .is_err()
        );
        let manifest_key = writer.finish().await.unwrap();
        let manifest = store
            .get(&manifest_key)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let manifest: Value = serde_json::from_slice(&manifest).unwrap();
        let chunk = &manifest["tracks"][0]["chunks"][0];
        let data = store
            .get(&Path::from(chunk["data"].as_str().unwrap()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let index = store
            .get(&Path::from(chunk["index"].as_str().unwrap()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let entries: Value = serde_json::from_slice(&index).unwrap();

        // Assert
        assert_eq!(&data[..], b"payload");
        assert_eq!(chunk["data_sha256"], digest(&data));
        assert_eq!(chunk["index_sha256"], digest(&index));
        assert_eq!(entries[0]["group"], u64::MAX.to_string());
        assert_eq!(entries[0]["offset"], "0");
        assert_eq!(entries[0]["length"], "7");
        assert!(entries[0]["subgroup"].is_null());
        let mut duplicate = ChunkWriter::new(store.clone(), prefix, "other".to_string());
        duplicate
            .push(
                "video",
                &Event::End {
                    group_id: 1,
                    subgroup_id: 2,
                },
            )
            .await
            .unwrap();
        assert!(duplicate.finish().await.is_err());
    }

    #[tokio::test]
    async fn keeps_the_first_duplicate_and_rejects_conflicting_payloads() {
        // Arrange
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let mut writer = ChunkWriter::new(store.clone(), Path::from("dedup"), "anon/a".into());
        let mut object = Object {
            location: Location {
                group_id: 1,
                object_id: 0,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 1,
            status: ObjectStatus::Normal,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(b"first"),
            ends_group: false,
        };
        // Act
        writer
            .push("chat", &Event::Object(object.clone()))
            .await
            .unwrap();
        writer
            .push("chat", &Event::Object(object.clone()))
            .await
            .unwrap();
        object.payload = Bytes::from_static(b"conflict");
        let conflict = writer.push("chat", &Event::Object(object)).await;
        // Assert
        assert!(conflict.is_err());
        assert_eq!(writer.tracks["chat"].entries.len(), 1);
        assert_eq!(writer.tracks["chat"].data, b"first");
    }

    #[tokio::test]
    async fn lists_the_first_object_of_each_group_with_its_timestamp() {
        // Arrange
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let mut writer = ChunkWriter::new(store.clone(), Path::from("starts"), "anon/a".into());
        let object = |group_id, object_id, timestamp_us| {
            let mut object = crate::test_support::recorded_object(group_id, timestamp_us);
            object.location.object_id = object_id;
            Event::Object(object)
        };
        for event in [
            object(3, 0, 2_000_000),
            object(3, 1, 2_033_000),
            object(4, 0, 3_000_000),
        ] {
            writer.push("video", &event).await.unwrap();
        }

        // Act
        let key = writer.finish().await.unwrap();
        let bytes = store.get(&key).await.unwrap().bytes().await.unwrap();
        let manifest: Value = serde_json::from_slice(&bytes).unwrap();

        // Assert
        let chunk = &manifest["tracks"][0]["chunks"][0];
        assert_eq!(
            chunk["groups"],
            json!([
                { "group": "3", "object": "0", "timestamp_us": "2000000" },
                { "group": "4", "object": "0", "timestamp_us": "3000000" },
            ])
        );
        assert_eq!(
            (&chunk["timestamp_start_us"], &chunk["timestamp_end_us"]),
            (&json!("2000000"), &json!("3000000"))
        );
    }

    #[tokio::test]
    async fn lists_the_groups_whose_media_a_chunk_holds_in_numeric_order() {
        // Arrange
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let mut writer = ChunkWriter::new(store.clone(), Path::from("groups"), "anon/a".into());
        let object = |group_id, status| {
            Event::Object(Object {
                location: Location {
                    group_id,
                    object_id: 0,
                },
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                publisher_priority: 1,
                status,
                extension_headers: ExtensionHeaders::default(),
                payload: if status == ObjectStatus::Normal {
                    Bytes::from_static(b"frame")
                } else {
                    Bytes::new()
                },
                ends_group: false,
            })
        };
        for event in [
            object(10, ObjectStatus::Normal),
            object(9, ObjectStatus::Normal),
            object(11, ObjectStatus::EndOfGroup),
        ] {
            writer.push("video", &event).await.unwrap();
        }

        // Act
        let key = writer.finish().await.unwrap();
        let bytes = store.get(&key).await.unwrap().bytes().await.unwrap();
        let manifest: Value = serde_json::from_slice(&bytes).unwrap();

        // Assert: group 11 holds only a status object
        assert_eq!(
            manifest["tracks"][0]["chunks"][0]["groups"],
            json!([
                { "group": "9", "object": "0", "timestamp_us": null },
                { "group": "10", "object": "0", "timestamp_us": null },
            ])
        );
    }

    #[tokio::test]
    async fn rotates_chunks_without_losing_object_boundaries() {
        // Arrange
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let mut writer =
            ChunkWriter::new(store.clone(), Path::from("rotation"), "anon/a".to_string());
        let object = |object_id, payload: Bytes| {
            Event::Object(Object {
                location: Location {
                    group_id: 4,
                    object_id,
                },
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                publisher_priority: 1,
                status: ObjectStatus::Normal,
                extension_headers: ExtensionHeaders::default(),
                payload,
                ends_group: false,
            })
        };

        // Act
        writer
            .push("video", &object(0, Bytes::from(vec![7; CHUNK_BYTES])))
            .await
            .unwrap();
        writer
            .push("video", &object(1, Bytes::from_static(b"next")))
            .await
            .unwrap();
        let key = writer.finish().await.unwrap();
        let bytes = store.get(&key).await.unwrap().bytes().await.unwrap();
        let manifest: Value = serde_json::from_slice(&bytes).unwrap();

        // Assert
        let chunks = manifest["tracks"][0]["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["data_size"], CHUNK_BYTES.to_string());
        assert_eq!(chunks[1]["data_size"], "4");
        let second = store
            .get(&Path::from(chunks[1]["data"].as_str().unwrap()))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(&second[..], b"next");
    }
}
