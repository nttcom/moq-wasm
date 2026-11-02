use std::{sync::Arc, time::SystemTime};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, VideoConfig, VideoSample, loc,
    mp4::{Demuxer, Mp4Index, SampleKind},
};
use moqt::object::{ForwardingPreference, Location, Object, ObjectStatus};
use msf::{
    Catalog, Track,
    types::{KnownPackaging, KnownTrackRole, TrackRole},
};
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path};
use serde_json::{Value, json};

use crate::{
    archive_chunks::{ChunkWriter, MAX_CHUNK_BYTES},
    archive_receiver::Event,
    manifest::parse_manifest,
    store::RecordingStore,
};

const READ_BYTES: u64 = 4 * 1024 * 1024;

/// Builds a reusable replay archive without changing the original MP4 files or manifest.
pub(crate) async fn import(
    store: &RecordingStore,
    id: &str,
    room: &str,
    manifest: &Value,
) -> Result<Value> {
    let objects = store.object_store();
    let prefix = store.recording_prefix(id);
    let cache = prefix.clone().join("mp4-replay/manifest.json");
    match objects.get(&cache).await {
        Ok(result) => {
            ensure!(
                result.meta.size <= READ_BYTES,
                "MP4 replay manifest is too large"
            );
            let bytes = result.bytes().await?;
            ensure!(
                bytes.len() as u64 <= READ_BYTES,
                "MP4 replay manifest is too large"
            );
            return Ok(serde_json::from_slice(&bytes)?);
        }
        Err(object_store::Error::NotFound { .. }) => {}
        Err(error) => return Err(error.into()),
    }
    let attempt = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    let output = prefix.clone().join(format!("mp4-replay/{attempt}"));
    let mut members = Vec::new();
    for (member_index, member) in parse_manifest(manifest)?.into_iter().enumerate() {
        ensure!(
            member.namespace.starts_with(&format!("{room}/")),
            "member is outside the recorded room"
        );
        let mut entry = json!({ "namespace": member.namespace, "camera": [], "screen_share": [] });
        for (content, files) in [
            ("camera", member.files),
            ("screen_share", member.screen_share),
        ] {
            for (file_index, (key, origin)) in files.into_iter().enumerate() {
                ensure!(
                    key.as_ref().starts_with(&format!("{prefix}/")),
                    "MP4 key is outside the recording"
                );
                let file = File::open(objects.clone(), key).await?;
                let chunks = ChunkWriter::new(
                    objects.clone(),
                    output
                        .clone()
                        .join(format!("{member_index}/{content}/{file_index}")),
                    member.namespace.clone(),
                );
                let mut writer = ImportWriter::new(chunks, origin);
                convert(file, &mut writer).await?;
                let key = writer.finish().await?;
                entry[content]
                    .as_array_mut()
                    .unwrap()
                    .push(json!(key.as_ref()));
            }
        }
        members.push(entry);
    }
    let manifest = json!({ "version": 1, "format": "moq", "members": members });
    let bytes = serde_json::to_vec(&manifest)?;
    ensure!(
        bytes.len() as u64 <= READ_BYTES,
        "MP4 replay manifest is too large"
    );
    objects.put(&cache, PutPayload::from(bytes)).await?;
    Ok(manifest)
}

struct File {
    store: Arc<dyn ObjectStore>,
    key: Path,
    size: u64,
    offset: u64,
    buffer: Bytes,
}

impl File {
    async fn open(store: Arc<dyn ObjectStore>, key: Path) -> Result<Self> {
        let size = store.head(&key).await?.size;
        Ok(Self {
            store,
            key,
            size,
            offset: 0,
            buffer: Bytes::new(),
        })
    }

    async fn read(&mut self, offset: u64, length: u64) -> Result<Bytes> {
        let end = offset.checked_add(length).context("MP4 range overflow")?;
        ensure!(end <= self.size, "MP4 range is outside the file");
        ensure!(
            length <= MAX_CHUNK_BYTES as u64,
            "MP4 sample or metadata exceeds 16 MiB"
        );
        if offset < self.offset || end > self.offset + self.buffer.len() as u64 {
            let limit = offset.saturating_add(READ_BYTES.max(length)).min(self.size);
            self.buffer = self.store.get_range(&self.key, offset..limit).await?;
            self.offset = offset;
        }
        let start = usize::try_from(offset - self.offset)?;
        Ok(self.buffer.slice(start..start + usize::try_from(length)?))
    }
}

async fn convert(mut file: File, writer: &mut ImportWriter) -> Result<()> {
    let (mut offset, mut fragmented, mut largest_atom) = (0u64, false, 0u64);
    let mut moov = None;
    while offset < file.size {
        let header = file.read(offset, 8).await?;
        let mut size = u64::from(u32::from_be_bytes(header[..4].try_into()?));
        let header_size = if size == 1 {
            size = u64::from_be_bytes(file.read(offset + 8, 8).await?[..].try_into()?);
            16
        } else {
            if size == 0 {
                size = file.size - offset;
            }
            8
        };
        ensure!(
            size >= header_size && size <= file.size - offset,
            "invalid MP4 atom size"
        );
        largest_atom = largest_atom.max(size);
        match &header[4..8] {
            b"moov" => {
                ensure!(moov.is_none(), "multiple MP4 initialization segments");
                ensure!(size <= READ_BYTES, "MP4 metadata exceeds 4 MiB");
                moov = Some(file.read(offset, size).await?);
            }
            b"moof" => fragmented = true,
            _ => {}
        }
        offset += size;
    }
    let moov = moov.context("MP4 has no moov atom")?;
    if fragmented {
        ensure!(
            largest_atom <= MAX_CHUNK_BYTES as u64,
            "MP4 fragment exceeds 16 MiB"
        );
        let mut demuxer = Demuxer::new();
        let mut offset = 0;
        while offset < file.size {
            let length = READ_BYTES.min(file.size - offset);
            for event in demuxer.push(&file.read(offset, length).await?)? {
                writer.push(event).await?;
            }
            offset += length;
        }
        for event in demuxer.finish()? {
            writer.push(event).await?;
        }
    } else {
        let index = Mp4Index::parse(&moov)?;
        ensure!(
            index.samples().len() <= 1_000_000,
            "MP4 exceeds one million samples"
        );
        if let Some(config) = &index.video {
            writer
                .push(MediaEvent::VideoConfig(VideoConfig::Avc(config.clone())))
                .await?;
        }
        if let Some(config) = &index.audio {
            writer.push(MediaEvent::AudioConfig(config.clone())).await?;
        }
        for sample in index.samples() {
            let bytes = file.read(sample.offset, u64::from(sample.size)).await?;
            let event = match sample.kind {
                SampleKind::Video => MediaEvent::Video(VideoSample {
                    data: index.annexb_video_sample(&bytes, sample.is_sync)?,
                    is_keyframe: sample.is_sync,
                    pts: sample.pts,
                    dts: sample.dts,
                }),
                SampleKind::Audio => MediaEvent::Audio(AudioSample {
                    data: bytes,
                    pts: sample.pts,
                    audio_level: None,
                }),
            };
            writer.push(event).await?;
        }
    }
    Ok(())
}

struct ImportWriter {
    chunks: ChunkWriter,
    loc: loc::Muxer,
    catalog: Catalog,
    video_group: u64,
    video_object: u64,
    audio_object: u64,
}

impl ImportWriter {
    fn new(chunks: ChunkWriter, origin: Timestamp) -> Self {
        Self {
            chunks,
            loc: loc::Muxer::new(origin),
            catalog: Catalog::new(Vec::new(), origin.millis()),
            video_group: 0,
            video_object: 0,
            audio_object: 0,
        }
    }

    async fn push(&mut self, event: MediaEvent) -> Result<()> {
        let (name, location) = match &event {
            MediaEvent::Streams(_) => return Ok(()),
            MediaEvent::VideoConfig(config) => {
                self.loc = self.loc.with_video_codec(loc::VideoCodec::from(config));
                let mut track = Track::new("video", KnownPackaging::Loc, false);
                track.role = Some(TrackRole::Known(KnownTrackRole::Video));
                track.codec = Some(config.codec_string());
                self.catalog.tracks.as_mut().unwrap().push(track);
                return Ok(());
            }
            MediaEvent::AudioConfig(config) => {
                let mut track = Track::new("audio", KnownPackaging::Loc, false);
                track.role = Some(TrackRole::Known(KnownTrackRole::Audio));
                track.codec = Some(config.codec_string());
                track.sample_rate = Some(config.sample_rate());
                track.channel_config = Some(match config.channels() {
                    1 => "mono".into(),
                    2 => "stereo".into(),
                    n => format!("{n}ch"),
                });
                if let AudioConfig::Aac(config) = config {
                    let init = track.set_inline_init_data(STANDARD.encode(config.to_bytes()));
                    self.catalog
                        .init_data_list
                        .get_or_insert_with(Vec::new)
                        .push(init);
                }
                self.catalog.tracks.as_mut().unwrap().push(track);
                return Ok(());
            }
            MediaEvent::Video(sample) => {
                if sample.is_keyframe {
                    self.video_group += 1;
                    self.video_object = 0;
                }
                if self.video_group == 0 {
                    return Ok(());
                }
                let location = Location {
                    group_id: self.video_group,
                    object_id: self.video_object,
                };
                self.video_object += 1;
                ("video", location)
            }
            MediaEvent::Audio(_) => {
                let location = Location {
                    group_id: 0,
                    object_id: self.audio_object,
                };
                self.audio_object += 1;
                ("audio", location)
            }
        };
        let loc = self
            .loc
            .push(&event)
            .context("MP4 sample produced no LOC object")?;
        let object = Object {
            location,
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 128,
            status: ObjectStatus::Normal,
            extension_headers: loc::to_extension_headers(&loc.extensions),
            payload: loc.payload,
            ends_group: false,
        };
        self.chunks.push(name, &Event::Object(object)).await
    }

    async fn finish(mut self) -> Result<Path> {
        ensure!(
            self.video_group > 0 || self.audio_object > 0,
            "MP4 has no playable samples"
        );
        let object = Object {
            location: Location {
                group_id: 0,
                object_id: 0,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 128,
            status: ObjectStatus::Normal,
            extension_headers: Default::default(),
            payload: Bytes::from(serde_json::to_vec(&self.catalog)?),
            ends_group: true,
        };
        self.chunks.push("catalog", &Event::Object(object)).await?;
        self.chunks.finish().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{archive_reader::StoredRecording, test_support::scratch_dir};
    use url::Url;

    const FMP4: &[u8] = include_bytes!("../../../crates/mediapack/fixtures/testsrc.mp4");
    const MP4: &[u8] = include_bytes!("../../../crates/mediapack/fixtures/testsrc_progressive.mp4");

    #[tokio::test]
    async fn imports_fragmented_and_progressive_mp4_and_reuses_the_completed_archive() {
        // Arrange
        for (id, data) in [("fragmented", FMP4), ("progressive", MP4)] {
            let dir = scratch_dir();
            let store = RecordingStore::open(&Url::from_directory_path(&dir).unwrap()).unwrap();
            let objects = store.object_store();
            let key = store.recording_prefix(id).join("source.mp4");
            let source = json!({ "members": [{
                "namespace": "anon/room/alice",
                "files": [{ "key": key.as_ref(), "capture_origin_us": 1_700_000_000_000_000u64 }]
            }] });
            objects
                .put(&key, PutPayload::from(Bytes::from_static(data)))
                .await
                .unwrap();
            objects
                .put(
                    &store.recording_prefix(id).join("manifest.json"),
                    PutPayload::from(serde_json::to_vec(&source).unwrap()),
                )
                .await
                .unwrap();

            // Act
            let archive = StoredRecording::open(&store, id, "anon/room")
                .await
                .unwrap();
            let reopened = StoredRecording::open(&store, id, "anon/room")
                .await
                .unwrap();

            // Assert
            assert_eq!(archive.description(), reopened.description());
            for name in ["video", "audio", "catalog"] {
                let track = archive
                    .tracks
                    .iter()
                    .find(|track| track.name == name)
                    .unwrap();
                let mut restored = Vec::new();
                for chunk in &track.chunks {
                    restored.extend(archive.objects(chunk).await.unwrap());
                }
                assert!(!restored.is_empty());
                if name == "catalog" {
                    let catalog: Catalog = serde_json::from_slice(&restored[0].payload).unwrap();
                    let audio = catalog
                        .loc_tracks()
                        .find(|track| track.name == "audio")
                        .unwrap();
                    assert!(catalog.init_data(audio).is_some());
                } else {
                    for object in restored {
                        let object = moqt::TrackObject::from_object(object).unwrap();
                        assert!(
                            crate::pipeline::capture_timestamp(&object)
                                .unwrap()
                                .micros()
                                >= 1_700_000_000_000_000
                        );
                    }
                }
            }
            assert_eq!(
                objects
                    .get(&key)
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap()
                    .as_ref(),
                data
            );
            assert_eq!(store.read_manifest(id).await.unwrap().unwrap(), source);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[tokio::test]
    async fn rejects_escaping_keys_and_retries_after_an_incomplete_import() {
        // Arrange
        let dir = scratch_dir();
        let store = RecordingStore::open(&Url::from_directory_path(&dir).unwrap()).unwrap();
        let objects = store.object_store();
        let key = store.recording_prefix("id").join("source.mp4");
        let mut source = json!({ "members": [{
            "namespace": "anon/room/alice",
            "files": [{ "key": "another-id/secret.mp4", "capture_origin_us": 1_700_000_000_000_000u64 }]
        }] });

        // Act / Assert
        assert!(
            import(&store, "id", "anon/room", &source)
                .await
                .unwrap_err()
                .to_string()
                .contains("outside")
        );
        source["members"][0]["files"][0]["key"] = json!(key.as_ref());
        objects
            .put(
                &key,
                PutPayload::from(Bytes::copy_from_slice(&FMP4[..FMP4.len() - 1])),
            )
            .await
            .unwrap();
        assert!(import(&store, "id", "anon/room", &source).await.is_err());
        assert!(
            objects
                .head(
                    &store
                        .recording_prefix("id")
                        .join("mp4-replay/manifest.json")
                )
                .await
                .is_err()
        );
        objects
            .put(&key, PutPayload::from(Bytes::from_static(FMP4)))
            .await
            .unwrap();
        assert_eq!(
            import(&store, "id", "anon/room", &source).await.unwrap()["format"],
            "moq"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
