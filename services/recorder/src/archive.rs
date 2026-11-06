use std::{path::PathBuf, task::Poll, time::SystemTime};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use moqt::{Session, TrackObject, subscribe::FilterType};
use serde_json::json;
use tokio::{
    fs::OpenOptions,
    io::{AsyncWriteExt, BufWriter},
    signal::unix::{SignalKind, signal},
};

use crate::{
    Args,
    archive_chunks::ChunkWriter,
    archive_receiver::{ArchiveTrack, Event},
    recorder::{BroadcastOptions, Content, choice, default_output_path},
    track_selection,
};

pub(crate) async fn record(session: &Session, args: &Args) -> Result<()> {
    let mut interrupt = signal(SignalKind::interrupt())?;
    record_until(session, args, async {
        let _ = interrupt.recv().await;
    })
    .await
}

async fn record_until(
    session: &Session,
    args: &Args,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let path = args.output.clone().unwrap_or_else(|| {
        default_output_path(&args.namespace, SystemTime::now()).with_extension("jsonl")
    });
    let chunks = args
        .upload
        .as_ref()
        .map(|url| {
            let (store, mut prefix) = object_store::parse_url_opts(url, std::env::vars())
                .with_context(|| format!("cannot use {url} as the upload location"))?;
            prefix.extend(args.namespace.split('/'));
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("archive output must have a UTF-8 file name")?;
            Ok::<_, anyhow::Error>(ChunkWriter::new(
                std::sync::Arc::from(store),
                prefix.join(name),
                args.namespace.clone(),
            ))
        })
        .transpose()?;
    let options = BroadcastOptions {
        content: Content::Camera,
        video_track: args.video_track.clone(),
        max_video_height: args.max_video_height,
        audio_track: args.audio_track.clone(),
        layout: args.layout,
        from_cache: false,
        segment_length: None,
        upload: None,
    };
    record_broadcast(session, &args.namespace, Some(path), &options, chunks, stop)
        .await
        .map(drop)
}

pub(crate) async fn record_broadcast(
    session: &Session,
    namespace: &str,
    path: Option<PathBuf>,
    options: &BroadcastOptions,
    chunks: Option<ChunkWriter>,
    stop: impl Future<Output = ()>,
) -> Result<Option<object_store::path::Path>> {
    tokio::pin!(stop);
    let mut archive = Archive::create(path, namespace, chunks).await?;
    let mut catalog = ArchiveTrack::subscribe(
        session,
        namespace,
        "catalog",
        FilterType::LargestObject,
        true,
    )
    .await?;
    let (mut video, mut audio): (Option<ArchiveTrack>, Option<ArchiveTrack>) = (None, None);
    let mut events: Vec<ArchiveTrack> = Vec::new();
    let mut catalog_open = true;
    loop {
        tokio::select! {
            _ = &mut stop => break,
            close = session.closed() => anyhow::bail!("relay session closed: {close}"),
            item = catalog.next(), if catalog_open => match item? {
                Some(event) => {
                    let document = match &event {
                        Event::Object(object) if object.status == moqt::object::ObjectStatus::Normal && !object.payload.is_empty() => {
                            Some(serde_json::from_slice::<msf::Catalog>(&object.payload).context("catalog is not MSF JSON")?)
                        }
                        _ => None,
                    };
                    archive.write_event("catalog", event).await?;
                    let Some(document) = document else { continue };
                    if options.content == Content::Camera {
                        subscribe_event_timelines(session, namespace, &document, &mut events).await?;
                    }
                    let selection = track_selection::select_tracks(&document, choice(options))?;
                    let video_name = selection.as_ref().and_then(|s| s.video.as_ref()).map(|v| v.name.as_str());
                    let audio_name = selection.as_ref().and_then(|s| s.audio.as_ref()).map(|a| a.name.as_str());
                    if video.as_ref().map(|t| t.subscription.track_name()) != video_name {
                        video = match video_name {
                            Some(name) => Some(ArchiveTrack::subscribe(session, namespace, name, FilterType::NextGroupStart, false).await?),
                            None => None,
                        };
                    }
                    if audio.as_ref().map(|t| t.subscription.track_name()) != audio_name {
                        audio = match audio_name {
                            Some(name) => Some(ArchiveTrack::subscribe(session, namespace, name, FilterType::LargestObject, false).await?),
                            None => None,
                        };
                    }
                }
                None => catalog_open = false,
            },
            item = next(&mut video) => match item? {
                Some(event) => if let Some(track) = &video {
                    archive.write_event(track.subscription.track_name(), event).await?;
                },
                None => video = None,
            },
            item = next(&mut audio) => match item? {
                Some(event) => if let Some(track) = &audio {
                    archive.write_event(track.subscription.track_name(), event).await?;
                },
                None => audio = None,
            },
            (index, item) = next_of(&mut events) => match item? {
                Some(event) => archive.write_event(events[index].subscription.track_name(), event).await?,
                None => drop(events.remove(index)),
            },
        }
        if !catalog_open && video.is_none() && audio.is_none() && events.is_empty() {
            break;
        }
    }
    archive.finish().await
}

pub(crate) async fn record_event_track(
    session: &Session,
    namespace: &str,
    name: &str,
    path: Option<PathBuf>,
    chunks: ChunkWriter,
    stop: impl Future<Output = ()>,
) -> Result<Option<object_store::path::Path>> {
    tokio::pin!(stop);
    let mut archive = Archive::create(path, namespace, Some(chunks)).await?;
    let mut track =
        ArchiveTrack::subscribe(session, namespace, name, FilterType::LargestObject, true).await?;
    loop {
        tokio::select! {
            _ = &mut stop => break,
            close = session.closed() => anyhow::bail!("relay session closed: {close}"),
            event = track.next() => match event? {
                Some(event) => archive.write_event(name, event).await?,
                None => break,
            }
        }
    }
    archive.finish().await
}

async fn next(track: &mut Option<ArchiveTrack>) -> Result<Option<Event>> {
    match track {
        Some(track) => track.next().await,
        None => std::future::pending().await,
    }
}

/// Subscribes each event timeline track that `catalog` lists and `events`
/// lacks, from its current group. A track the relay does not know yet is
/// tried again at the next catalog.
async fn subscribe_event_timelines(
    session: &Session,
    namespace: &str,
    catalog: &msf::Catalog,
    events: &mut Vec<ArchiveTrack>,
) -> Result<()> {
    for timeline in track_selection::event_timelines(catalog) {
        let name = timeline.name.as_str();
        if events
            .iter()
            .any(|track| track.subscription.track_name() == name)
        {
            continue;
        }
        match ArchiveTrack::subscribe(session, namespace, name, FilterType::LargestObject, true)
            .await
        {
            Ok(track) => events.push(track),
            Err(error)
                if matches!(
                    error.downcast_ref::<moqt::RequestFailure>(),
                    Some(moqt::RequestFailure::Rejected {
                        code: moqt::RequestErrorCode::TrackDoesNotExist,
                        ..
                    })
                ) =>
            {
                tracing::debug!(
                    track_namespace = namespace,
                    track_name = name,
                    "event timeline track does not exist yet"
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// The next event of any of `tracks`, with its track's index; pending while
/// `tracks` is empty.
async fn next_of(tracks: &mut [ArchiveTrack]) -> (usize, Result<Option<Event>>) {
    std::future::poll_fn(|cx| {
        tracks
            .iter_mut()
            .enumerate()
            .find_map(|(index, track)| match track.poll_next(cx) {
                Poll::Ready(event) => Some((index, event)),
                Poll::Pending => None,
            })
            .map_or(Poll::Pending, Poll::Ready)
    })
    .await
}

/// Writes to the local JSON Lines file, the object store chunks, or both.
struct Archive {
    file: Option<BufWriter<tokio::fs::File>>,
    chunks: Option<ChunkWriter>,
}

impl Archive {
    async fn create(
        path: Option<PathBuf>,
        namespace: &str,
        chunks: Option<ChunkWriter>,
    ) -> Result<Self> {
        let file = match path {
            Some(path) => Some(BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .await
                    .with_context(|| format!("cannot create {}", path.display()))?,
            )),
            None => None,
        };
        let mut archive = Self { file, chunks };
        archive
            .line(json!({"version": 1, "namespace": namespace}))
            .await?;
        Ok(archive)
    }

    async fn write_event(&mut self, track: &str, event: Event) -> Result<()> {
        if let Some(chunks) = &mut self.chunks {
            chunks.push(track, &event).await?;
        }
        match event {
            Event::End {
                group_id,
                subgroup_id,
            } => {
                self.line(json!({
                    "track": track,
                    "end": { "group": group_id.to_string(), "subgroup": subgroup_id.to_string() },
                }))
                .await
            }
            Event::Header(header) => {
                self.line(json!({
                    "track": track,
                    "header": {
                        "group": header.group_id.to_string(),
                        "subgroup": header.subgroup_id.to_string(),
                        "priority": header.publisher_priority,
                        "ends_group": header.ends_group,
                    },
                }))
                .await
            }
            Event::Object(object) => {
                let subgroup = match object.forwarding {
                    moqt::object::ForwardingPreference::Subgroup { subgroup_id } => {
                        Some(subgroup_id.to_string())
                    }
                    moqt::object::ForwardingPreference::Datagram => None,
                };
                let capture = TrackObject::from_object(object.clone())
                    .as_ref()
                    .and_then(crate::pipeline::capture_timestamp)
                    .map(|timestamp| timestamp.micros().to_string());
                self.line(json!({
                    "track": track,
                    "group": object.location.group_id.to_string(),
                    "subgroup": subgroup,
                    "object": object.location.object_id.to_string(),
                    "priority": object.publisher_priority,
                    "status": object.status as u8,
                    "ends_group": object.ends_group,
                    "extensions": STANDARD.encode(object.extension_headers.encode()),
                    "payload": STANDARD.encode(&object.payload),
                    "capture_us": capture,
                }))
                .await
            }
        }
    }

    async fn line(&mut self, value: serde_json::Value) -> Result<()> {
        let Some(file) = &mut self.file else {
            return Ok(());
        };
        file.write_all(&serde_json::to_vec(&value)?).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok(())
    }

    async fn finish(mut self) -> Result<Option<object_store::path::Path>> {
        let key = if let Some(chunks) = self.chunks.take() {
            let key = chunks.finish().await?;
            tracing::info!(key = %key, "archive manifest written");
            Some(key)
        } else {
            None
        };
        self.line(json!({"complete": true})).await?;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use clap::Parser;
    use moqt::object::ExtensionHeaders;
    use moqt::publish::{GroupBoundary, OutgoingObject, Retention};
    use msf::{Catalog, KnownPackaging, KnownTrackRole};
    use std::time::Duration;

    use super::*;
    use crate::test_support::scratch_dir;

    #[tokio::test]
    async fn stores_object_identity_headers_and_payload_without_overwriting() {
        // Arrange
        let dir = scratch_dir();
        let path = dir.join("archive.jsonl");
        let mut headers = ExtensionHeaders::default();
        headers.push_immutable_extension(Bytes::from_static(b"header"));
        let object = moqt::object::Object {
            location: moqt::object::Location {
                group_id: u64::MAX,
                object_id: 3,
            },
            forwarding: moqt::object::ForwardingPreference::Subgroup { subgroup_id: 2 },
            publisher_priority: 0,
            status: moqt::object::ObjectStatus::Normal,
            extension_headers: headers.clone(),
            payload: Bytes::from_static(b"frame"),
            ends_group: false,
        };
        let mut archive = Archive::create(Some(path.clone()), "anon/test", None)
            .await
            .unwrap();

        // Act
        archive
            .write_event("video", Event::Object(object))
            .await
            .unwrap();
        archive.finish().await.unwrap();
        let bytes = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<serde_json::Value> = bytes
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        // Assert
        assert_eq!(lines[0]["namespace"], "anon/test");
        assert_eq!(lines[1]["group"], u64::MAX.to_string());
        assert_eq!(
            STANDARD
                .decode(lines[1]["payload"].as_str().unwrap())
                .unwrap(),
            b"frame"
        );
        let encoded = STANDARD
            .decode(lines[1]["extensions"].as_str().unwrap())
            .unwrap();
        assert_eq!(ExtensionHeaders::decode(&mut &encoded[..]), Ok(headers));
        assert_eq!(lines[2]["complete"], true);
        assert!(Archive::create(Some(path), "other", None).await.is_err());
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn stores_datagram_status_and_subgroup_header() {
        // Arrange
        use moqt::object::{ForwardingPreference, Location, Object, ObjectStatus, SubgroupStart};
        let dir = scratch_dir();
        let path = dir.join("archive.jsonl");
        let mut archive = Archive::create(Some(path.clone()), "anon/test", None)
            .await
            .unwrap();
        let header = SubgroupStart {
            group_id: 9,
            subgroup_id: 7,
            publisher_priority: 11,
            ends_group: true,
        };
        let object = Object {
            location: Location {
                group_id: 9,
                object_id: 2,
            },
            forwarding: ForwardingPreference::Datagram,
            publisher_priority: 42,
            status: ObjectStatus::EndOfGroup,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::new(),
            ends_group: false,
        };

        // Act
        archive
            .write_event("video", Event::Header(header))
            .await
            .unwrap();
        archive
            .write_event("video", Event::Object(object))
            .await
            .unwrap();
        archive.finish().await.unwrap();
        let text = tokio::fs::read_to_string(path).await.unwrap();
        let entries: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        // Assert
        assert_eq!(entries[1]["header"]["subgroup"], "7");
        assert_eq!(entries[1]["header"]["ends_group"], true);
        assert_eq!(entries[2]["group"], "9");
        assert_eq!(entries[2]["object"], "2");
        assert_eq!(entries[2]["priority"], 42);
        assert_eq!(entries[2]["status"], 3);
        assert!(entries[2]["subgroup"].is_null());
        assert_eq!(entries[2]["payload"], "");
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn records_the_catalog_the_video_and_the_listed_chat_over_a_real_session() {
        // Arrange
        let dir = scratch_dir();
        let path = dir.join("archive.jsonl");
        let mut publisher = crate::test_support::FakePublisher::spawn();
        let namespace = "anon/test";
        let origin = publisher.origin.clone();
        origin
            .ensure_track(namespace, "catalog", 0, Retention::NewestOnly)
            .await;
        origin
            .ensure_track(namespace, "video", 0, Retention::NewestOnly)
            .await;
        origin
            .ensure_track(namespace, "chat", 0, Retention::NewestOnly)
            .await;
        let mut video = msf::Track::new("video", KnownPackaging::Loc, true);
        video.role = Some(KnownTrackRole::Video.into());
        video.codec = Some("avc1.42C01E".into());
        let mut chat = msf::Track::new("chat", KnownPackaging::EventTimeline, true);
        chat.depends = Some(vec!["video".into()]);
        let catalog = serde_json::to_vec(&Catalog::new(vec![video, chat], 0)).unwrap();
        origin
            .write(
                namespace,
                "catalog",
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from(catalog)),
            )
            .await
            .unwrap();
        origin
            .write(
                namespace,
                "chat",
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from_static(b"hello")),
            )
            .await
            .unwrap();
        let connection = crate::relay::RelayConnection::connect(&publisher.url, false, None)
            .await
            .unwrap();
        let args = Args::parse_from([
            "recorder",
            "--relay",
            &publisher.url,
            "--namespace",
            namespace,
            "--archive",
            "--output",
            path.to_str().unwrap(),
        ]);
        let (stop_sender, stop) = tokio::sync::oneshot::channel();
        let session = connection.session.clone();
        let recording = tokio::spawn(async move {
            record_until(&session, &args, async {
                let _ = stop.await;
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while publisher.served.recv().await.as_deref() != Some("video") {}
        })
        .await
        .unwrap();

        // Act
        let frame = crate::test_support::fixture().video.remove(0);
        origin
            .write(
                namespace,
                "video",
                OutgoingObject {
                    group: GroupBoundary::Next,
                    extension_headers: frame.extension_headers.clone(),
                    payload: frame.payload.clone(),
                },
            )
            .await
            .unwrap();
        origin
            .write(
                namespace,
                "chat",
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from_static(b"later")),
            )
            .await
            .unwrap();
        let entries = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let data = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                let entries: Vec<serde_json::Value> = data
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                if entries
                    .iter()
                    .any(|entry| entry["track"] == "video" && entry["payload"].is_string())
                    && entries
                        .iter()
                        .filter(|entry| entry["track"] == "chat" && entry["payload"].is_string())
                        .count()
                        == 2
                {
                    break entries;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let _ = stop_sender.send(());
        recording.await.unwrap().unwrap();

        // Assert
        assert!(entries.iter().any(|entry| entry["track"] == "catalog"));
        let chat = entries
            .iter()
            .find(|entry| entry["track"] == "chat" && entry["payload"].is_string())
            .unwrap();
        assert_eq!(
            STANDARD.decode(chat["payload"].as_str().unwrap()).unwrap(),
            b"hello"
        );
        let stored = entries
            .iter()
            .find(|entry| entry["track"] == "video" && entry["payload"].is_string())
            .unwrap();
        assert_eq!(
            STANDARD
                .decode(stored["payload"].as_str().unwrap())
                .unwrap(),
            frame.payload.to_vec()
        );
        assert_eq!(
            STANDARD
                .decode(stored["extensions"].as_str().unwrap())
                .unwrap(),
            frame.extension_headers.encode().to_vec()
        );
        let final_data = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(final_data.lines().any(|line| line == "{\"complete\":true}"));
        connection.close().await;
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[tokio::test]
    async fn records_the_current_and_later_transcript_objects() {
        // Arrange
        let dir = scratch_dir();
        let path = dir.join("transcript.jsonl");
        let mut publisher = crate::test_support::FakePublisher::spawn();
        let namespace = "anon/room/bots/transcriber";
        let origin = publisher.origin.clone();
        origin
            .ensure_track(namespace, "transcript", 0, Retention::NewestOnly)
            .await;
        origin
            .write(
                namespace,
                "transcript",
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from_static(b"first")),
            )
            .await
            .unwrap();
        let connection = crate::relay::RelayConnection::connect(&publisher.url, false, None)
            .await
            .unwrap();
        let store: std::sync::Arc<dyn object_store::ObjectStore> =
            std::sync::Arc::new(object_store::memory::InMemory::new());
        let chunks = ChunkWriter::new(
            store,
            object_store::path::Path::from("transcript"),
            namespace.to_string(),
        );
        let (stop_sender, stop) = tokio::sync::oneshot::channel();
        let session = connection.session.clone();
        let output_path = path.clone();
        let recording = tokio::spawn(async move {
            record_event_track(
                &session,
                namespace,
                "transcript",
                Some(output_path),
                chunks,
                async {
                    let _ = stop.await;
                },
            )
            .await
        });
        assert_eq!(publisher.served.recv().await.as_deref(), Some("transcript"));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if tokio::fs::read_to_string(&path)
                    .await
                    .unwrap_or_default()
                    .contains("Zmlyc3Q=")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        // Act
        origin
            .write(
                namespace,
                "transcript",
                OutgoingObject::plain(GroupBoundary::Within, Bytes::from_static(b"second")),
            )
            .await
            .unwrap();
        let entries = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                let entries: Vec<serde_json::Value> = text
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                if entries
                    .iter()
                    .filter(|entry| entry["payload"].is_string())
                    .count()
                    == 2
                {
                    break entries;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let _ = stop_sender.send(());
        recording.await.unwrap().unwrap();

        // Assert
        let payloads: Vec<Vec<u8>> = entries
            .iter()
            .filter_map(|entry| entry["payload"].as_str())
            .map(|value| STANDARD.decode(value).unwrap())
            .collect();
        assert_eq!(payloads, [b"first".to_vec(), b"second".to_vec()]);
        connection.close().await;
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }
}
