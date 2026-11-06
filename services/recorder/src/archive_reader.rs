use std::{collections::BTreeSet, sync::Arc};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use moqt::object::{ExtensionHeaders, ForwardingPreference, Location, Object, ObjectStatus};
use object_store::{ObjectStore, ObjectStoreExt, path::Path};
use serde_json::Value;

use crate::{
    archive_chunks::{MAX_CHUNK_BYTES, digest},
    store::RecordingStore,
};

const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_TRACKS: usize = 256;

pub(crate) struct StoredTrack {
    pub(crate) namespace: String,
    pub(crate) source_namespace: String,
    pub(crate) name: String,
    pub(crate) chunks: Vec<Value>,
    pub(crate) first: Location,
    pub(crate) last: Location,
}

impl StoredTrack {
    /// The groups the track holds media of, as the archive manifest lists
    /// them.
    pub(crate) fn groups(&self) -> Result<BTreeSet<u64>> {
        let mut groups = BTreeSet::new();
        for chunk in &self.chunks {
            for group in chunk["groups"]
                .as_array()
                .context("archive chunk lists no groups")?
            {
                let group = group.as_str().context("group id is not a string")?;
                groups.insert(group.parse().context("invalid group id")?);
            }
        }
        Ok(groups)
    }

    /// The earliest and the latest LOC capture timestamp of the track's
    /// objects; `None` for a track whose objects carry none.
    pub(crate) fn capture_range_us(&self) -> Option<(u64, u64)> {
        let bounds = |field: &'static str| {
            self.chunks
                .iter()
                .filter_map(move |chunk| chunk[field].as_str()?.parse::<u64>().ok())
        };
        Some((
            bounds("capture_start_us").min()?,
            bounds("capture_end_us").max()?,
        ))
    }
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
        let manifest: Value = serde_json::from_slice(
            &recording
                .read(
                    store.recording_prefix(id).join("manifest.json").as_ref(),
                    MAX_MANIFEST_BYTES,
                )
                .await?,
        )?;
        ensure!(
            manifest["version"] == 1 && manifest["format"] == "moq",
            "unsupported recording format"
        );
        let members = manifest["members"]
            .as_array()
            .context("recording has no members")?;
        for member in members {
            let source_namespace = string(member, "namespace")?;
            let replay_prefix = member_replay_prefix(room, id, source_namespace)?;
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
                            namespace: format!("{replay_prefix}/{content}/{source_index}"),
                            source_namespace: source_namespace.to_string(),
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

    /// The replay namespaces, in the order the tracks list them.
    pub(crate) fn namespaces(&self) -> Vec<&str> {
        let mut namespaces: Vec<&str> = Vec::new();
        for track in &self.tracks {
            if !namespaces.contains(&track.namespace.as_str()) {
                namespaces.push(&track.namespace);
            }
        }
        namespaces
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

    /// The payloads of the track's Normal objects, in location order.
    pub(crate) async fn payloads(&self, track: &StoredTrack) -> Result<Vec<Bytes>> {
        let mut payloads = Vec::new();
        let mut cached_chunk = None;
        let mut data = Bytes::new();
        for entry in self.entries(track, track.first, track.last.next()).await? {
            if cached_chunk != Some(entry.chunk) {
                data = self.data(&track.chunks[entry.chunk]).await?;
                cached_chunk = Some(entry.chunk);
            }
            let object = decode_object(&entry.value, &data)?;
            if object.status == ObjectStatus::Normal {
                payloads.push(object.payload);
            }
        }
        Ok(payloads)
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
                if in_fetch_range(at, start, end) {
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

/// `<room>/recordings/<id>/<member>`, the prefix of the room member
/// `namespace`'s replay namespaces, where `<member>` is every element after
/// `room` (`alice`, `bots/transcriber`), so a participant named `transcriber`
/// stays apart from the bot. Fails when `namespace` is not below `room`.
fn member_replay_prefix(room: &str, id: &str, namespace: &str) -> Result<String> {
    let member = namespace
        .strip_prefix(room)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|member| !member.is_empty())
        .context("member is outside the recorded room")?;
    Ok(format!("{room}/recordings/{id}/{member}"))
}

/// Whether a FETCH of `[start, end)` covers `at`; an `end` with object id 0
/// takes its whole group (draft-14 §9.16.1).
pub(crate) fn in_fetch_range(at: Location, start: Location, end: Location) -> bool {
    at >= start && (at < end || (at.group_id == end.group_id && end.object_id == 0))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archive_chunks::ChunkWriter, archive_receiver::Event, test_support::recorded_camera,
    };
    use object_store::{PutPayload, memory::InMemory};
    use serde_json::json;

    #[test]
    fn a_member_replays_under_its_elements_after_the_room() {
        // Arrange: participants named transcriber and bots besides the transcriber bot
        let members = [
            "anon/room/alice",
            "anon/room/transcriber",
            "anon/room/bots",
            "anon/room/bots/transcriber",
        ];

        // Act
        let prefixes: Vec<String> = members
            .iter()
            .map(|member| member_replay_prefix("anon/room", "id", member).unwrap())
            .collect();

        // Assert
        assert_eq!(
            prefixes,
            [
                "anon/room/recordings/id/alice",
                "anon/room/recordings/id/transcriber",
                "anon/room/recordings/id/bots",
                "anon/room/recordings/id/bots/transcriber",
            ]
        );
    }

    #[tokio::test]
    async fn a_stored_archive_replays_under_its_member_content_and_index() {
        // Act
        let recording = recorded_camera().await;

        // Assert
        assert_eq!(
            recording.namespaces(),
            ["anon/room/recordings/id/alice/camera/0"]
        );
    }

    #[test]
    fn a_member_outside_the_room_is_rejected() {
        // Act / Assert
        for member in ["anon/room2/alice", "anon/room", "anon/room/", "anon/other"] {
            assert!(
                member_replay_prefix("anon/room", "id", member).is_err(),
                "{member}"
            );
        }
    }

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
