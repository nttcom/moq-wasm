use std::{
    path::{Path, PathBuf},
    sync::Arc,
    task::Poll,
    time::SystemTime,
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use moqt::{Session, TrackObject, object::ObjectStatus, subscribe::FilterType};
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
    recording_clock::RecordingClock,
    track_selection::{self, Selection},
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
                Arc::from(store),
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
    record_broadcast(
        session,
        &args.namespace,
        Some(path),
        &options,
        chunks,
        None,
        stop,
    )
    .await
    .map(drop)
}

fn part_path(path: &Path, index: usize) -> PathBuf {
    if index == 0 {
        return path.to_path_buf();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let name = match path.extension() {
        Some(extension) => format!("{stem}-{index}.{}", extension.to_string_lossy()),
        None => format!("{stem}-{index}"),
    };
    path.with_file_name(name)
}

/// Records the broadcast as one archive per stretch in which no recorded
/// track was replaced, so each archive's replay lists one track per kind: a
/// catalog that swaps a video or audio track the archive holds media of for
/// another, or changes its video codec or audio configuration, ends the
/// archive, and the next one starts from that catalog. Archive `index`
/// after the first goes to `path` and `chunks` with `-<index>` added
/// (`part_path`, `ChunkWriter::part`). With a `clock`, the archives store the
/// media and the event timelines on it. Returns the uploaded keys in archive
/// order.
pub(crate) async fn record_broadcast(
    session: &Session,
    namespace: &str,
    path: Option<PathBuf>,
    options: &BroadcastOptions,
    chunks: Option<ChunkWriter>,
    clock: Option<RecordingClock>,
    stop: impl Future<Output = ()>,
) -> Result<Vec<object_store::path::Path>> {
    tokio::pin!(stop);
    let mut keys = Vec::new();
    let mut index = 0;
    loop {
        let part_path = path.as_deref().map(|path| part_path(path, index));
        let part_chunks = chunks.as_ref().map(|chunks| chunks.part(index));
        let (key, end) = record_part(
            session,
            namespace,
            part_path,
            options,
            part_chunks,
            clock,
            &mut stop,
        )
        .await?;
        keys.extend(key);
        if end == PartEnd::Done {
            return Ok(keys);
        }
        index += 1;
    }
}

#[derive(Debug, PartialEq)]
enum PartEnd {
    Done,
    Replaced,
}

async fn record_part(
    session: &Session,
    namespace: &str,
    path: Option<PathBuf>,
    options: &BroadcastOptions,
    chunks: Option<ChunkWriter>,
    clock: Option<RecordingClock>,
    mut stop: impl Future<Output = ()> + Unpin,
) -> Result<(Option<object_store::path::Path>, PartEnd)> {
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
    let mut selected = Selection::default();
    // The video and audio track the archive holds media of.
    let mut recorded = Selection::default();
    let mut event_timelines: Vec<ArchiveTrack> = Vec::new();
    let (mut video_clock, mut audio_clock) = (MediaOnClock::new(clock), MediaOnClock::new(clock));
    let mut catalog_open = true;
    let mut end = PartEnd::Done;
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
                    let selection = document
                        .as_ref()
                        .map(|document| track_selection::select_tracks(document, choice(options)))
                        .transpose()?
                        .map(Option::unwrap_or_default);
                    if selection.as_ref().is_some_and(|selection| replaces(&recorded, selection)) {
                        end = PartEnd::Replaced;
                        break;
                    }
                    archive.write_event("catalog", event).await?;
                    let (Some(document), Some(selection)) = (document, selection) else { continue };
                    if options.content == Content::Camera {
                        let names = track_selection::event_timelines(&document).map(|track| track.name.as_str());
                        subscribe_event_timelines(session, namespace, names, &mut event_timelines).await?;
                    }
                    let video_name = selection.video.as_ref().map(|v| v.name.as_str());
                    let audio_name = selection.audio.as_ref().map(|a| a.name.as_str());
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
                    selected = selection;
                }
                None => catalog_open = false,
            },
            item = next(&mut video) => match item? {
                Some(event) => if let (Some(track), Some(event)) = (&video, video_clock.store(event)) {
                    if matches!(event, Event::Object(_)) && recorded.video.is_none() {
                        recorded.video = selected.video.clone();
                    }
                    archive.write_event(track.subscription.track_name(), event).await?;
                },
                None => video = None,
            },
            item = next(&mut audio) => match item? {
                Some(event) => if let (Some(track), Some(event)) = (&audio, audio_clock.store(event)) {
                    if matches!(event, Event::Object(_)) && recorded.audio.is_none() {
                        recorded.audio = selected.audio.clone();
                    }
                    archive.write_event(track.subscription.track_name(), event).await?;
                },
                None => audio = None,
            },
            (index, item) = next_of(&mut event_timelines) => match item? {
                Some(event) => archive.write_event(event_timelines[index].subscription.track_name(), event_timeline_on_clock(clock, event)).await?,
                None => drop(event_timelines.remove(index)),
            },
        }
        if !catalog_open && video.is_none() && audio.is_none() && event_timelines.is_empty() {
            break;
        }
    }
    Ok((archive.finish().await?, end))
}

/// Stores a media track's objects on the recording's clock a group at a time:
/// once an object of a group was captured before the recording started, the
/// rest of that group is left out too, so no stored group misses the object it
/// starts with, which for video is the keyframe. A publisher whose clock runs
/// behind the recorder's loses its first group this way.
struct MediaOnClock {
    clock: Option<RecordingClock>,
    dropped_group: Option<u64>,
}

impl MediaOnClock {
    fn new(clock: Option<RecordingClock>) -> Self {
        Self {
            clock,
            dropped_group: None,
        }
    }

    fn store(&mut self, event: Event) -> Option<Event> {
        match (self.clock, event) {
            (Some(clock), Event::Object(object)) => {
                let group_id = object.location.group_id;
                if self.dropped_group == Some(group_id) {
                    return None;
                }
                let stored = clock.media_object(object);
                if stored.is_none() {
                    self.dropped_group = Some(group_id);
                }
                stored.map(Event::Object)
            }
            (_, event) => Some(event),
        }
    }
}

fn event_timeline_on_clock(clock: Option<RecordingClock>, event: Event) -> Event {
    match (clock, event) {
        (Some(clock), Event::Object(mut object)) if object.status == ObjectStatus::Normal => {
            object.payload = clock.event_timeline(&object.payload);
            Event::Object(object)
        }
        (_, event) => event,
    }
}

fn replaces(recorded: &Selection, selection: &Selection) -> bool {
    fn swapped<T: PartialEq>(recorded: &Option<T>, next: &Option<T>) -> bool {
        matches!((recorded, next), (Some(recorded), Some(next)) if recorded != next)
    }
    swapped(&recorded.video, &selection.video) || swapped(&recorded.audio, &selection.audio)
}

pub(crate) async fn record_event_track(
    session: &Session,
    namespace: &str,
    name: &str,
    path: Option<PathBuf>,
    chunks: ChunkWriter,
    clock: RecordingClock,
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
                Some(event) => archive.write_event(name, event_timeline_on_clock(Some(clock), event)).await?,
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

/// Subscribes each of the event timeline tracks `names` that
/// `timelines` lacks, from its current group. A track the relay does not
/// know yet is tried again at the next catalog.
async fn subscribe_event_timelines<'a>(
    session: &Session,
    namespace: &str,
    names: impl Iterator<Item = &'a str>,
    timelines: &mut Vec<ArchiveTrack>,
) -> Result<()> {
    for name in names {
        if timelines
            .iter()
            .any(|track| track.subscription.track_name() == name)
        {
            continue;
        }
        match ArchiveTrack::subscribe(session, namespace, name, FilterType::LargestObject, true)
            .await
        {
            Ok(track) => timelines.push(track),
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
                    "timeline track does not exist yet"
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
    use mediapack::loc::VideoCodec;

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
    async fn records_the_catalog_the_video_and_the_listed_chat_but_no_media_timeline_over_a_real_session()
     {
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
        origin
            .ensure_track(namespace, "video_timeline", 0, Retention::NewestOnly)
            .await;
        let mut video = msf::Track::new("video", KnownPackaging::Loc, true);
        video.role = Some(KnownTrackRole::Video.into());
        video.codec = Some("avc1.42C01E".into());
        let mut chat = msf::Track::new("chat", KnownPackaging::EventTimeline, true);
        chat.depends = Some(vec!["video".into()]);
        let mut timeline = msf::Track::new("video_timeline", KnownPackaging::MediaTimeline, true);
        timeline.depends = Some(vec!["video".into()]);
        let catalog = serde_json::to_vec(&Catalog::new(vec![video, chat, timeline], 0)).unwrap();
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
        origin
            .write(
                namespace,
                "video_timeline",
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from_static(b"[[0,[1,0],1]]")),
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
        assert!(
            !entries
                .iter()
                .any(|entry| entry["track"] == "video_timeline")
        );
        let final_data = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(final_data.lines().any(|line| line == "{\"complete\":true}"));
        connection.close().await;
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[test]
    fn a_group_that_began_before_the_recording_is_left_out_whole() {
        // Arrange: the publisher's clock runs behind, so group 4 began before the start
        let mut clock = MediaOnClock::new(Some(RecordingClock::started_at(1_000)));
        let object = |group_id, object_id, capture_us| {
            use mediapack::loc::{LocExtension, TIMESTAMP_ID, to_extension_headers};

            let mut object = crate::test_support::stored_object(group_id, object_id, "frame");
            object.extension_headers =
                to_extension_headers(&[LocExtension::varint(TIMESTAMP_ID, capture_us)]);
            Event::Object(object)
        };

        // Act
        let stored: Vec<bool> = [
            object(4, 0, 999_000),
            object(4, 1, 1_033_000),
            object(5, 0, 1_066_000),
        ]
        .into_iter()
        .map(|event| clock.store(event).is_some())
        .collect();

        // Assert
        assert_eq!(stored, [false, false, true]);
    }

    #[test]
    fn a_swapped_track_or_codec_replaces_and_an_added_or_dropped_one_does_not() {
        // Arrange
        let video = |name: &str, codec| crate::track_selection::VideoTrack {
            name: name.to_string(),
            codec,
        };
        let recorded = Selection {
            video: Some(video("video_480p", VideoCodec::H264)),
            audio: None,
        };
        let with = |video| Selection { video, audio: None };

        // Act / Assert
        assert!(replaces(
            &recorded,
            &with(Some(video("video_720p", VideoCodec::H264)))
        ));
        assert!(replaces(
            &recorded,
            &with(Some(video("video_480p", VideoCodec::Vp8)))
        ));
        assert!(!replaces(
            &recorded,
            &with(Some(video("video_480p", VideoCodec::H264)))
        ));
        assert!(!replaces(&recorded, &with(None)));
        assert!(!replaces(&with(None), &recorded));
    }

    #[test]
    fn a_later_archive_takes_its_index_before_the_extension() {
        // Act / Assert
        assert_eq!(
            part_path(Path::new("out/a.jsonl"), 0),
            Path::new("out/a.jsonl")
        );
        assert_eq!(
            part_path(Path::new("out/a.jsonl"), 2),
            Path::new("out/a-2.jsonl")
        );
        assert_eq!(part_path(Path::new("out/a"), 1), Path::new("out/a-1"));
    }

    /// A recorder writing `archive.jsonl` from a publisher whose catalog
    /// lists `video_480p`, which also serves `video_720p`.
    struct RenditionSwitch {
        dir: PathBuf,
        publisher: crate::test_support::FakePublisher,
        connection: crate::relay::RelayConnection,
        stop_sender: tokio::sync::oneshot::Sender<()>,
        recording: tokio::task::JoinHandle<Result<()>>,
    }

    const SWITCH_NAMESPACE: &str = "anon/test";

    impl RenditionSwitch {
        async fn start() -> Self {
            let dir = scratch_dir();
            let publisher = crate::test_support::FakePublisher::spawn();
            for name in ["catalog", "video_480p", "video_720p"] {
                publisher
                    .origin
                    .ensure_track(SWITCH_NAMESPACE, name, 0, Retention::NewestOnly)
                    .await;
            }
            let switch_args = Args::parse_from([
                "recorder",
                "--relay",
                &publisher.url,
                "--namespace",
                SWITCH_NAMESPACE,
                "--archive",
                "--output",
                dir.join("archive.jsonl").to_str().unwrap(),
            ]);
            let connection = crate::relay::RelayConnection::connect(&publisher.url, false, None)
                .await
                .unwrap();
            publish_rendition_catalog(&publisher.origin, &[480]).await;
            let (stop_sender, stop) = tokio::sync::oneshot::channel();
            let session = connection.session.clone();
            let recording = tokio::spawn(async move {
                record_until(&session, &switch_args, async {
                    let _ = stop.await;
                })
                .await
            });
            let mut switch = Self {
                dir,
                publisher,
                connection,
                stop_sender,
                recording,
            };
            switch.served("video_480p").await;
            switch
        }

        async fn served(&mut self, name: &str) {
            tokio::time::timeout(Duration::from_secs(5), async {
                while self.publisher.served.recv().await.as_deref() != Some(name) {}
            })
            .await
            .unwrap()
        }

        /// Writes a frame on `track` and waits until archive `file` stores it.
        async fn write_frame(&self, track: &str, file: &str) {
            let frame = crate::test_support::fixture().video.remove(0);
            self.publisher
                .origin
                .write(
                    SWITCH_NAMESPACE,
                    track,
                    OutgoingObject {
                        group: GroupBoundary::Next,
                        extension_headers: frame.extension_headers,
                        payload: frame.payload,
                    },
                )
                .await
                .unwrap();
            let path = self.dir.join(file);
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let data = tokio::fs::read_to_string(&path).await.unwrap_or_default();
                    if data.lines().any(|line| {
                        serde_json::from_str::<serde_json::Value>(line).is_ok_and(|entry| {
                            entry["track"] == track && entry["payload"].is_string()
                        })
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap()
        }

        /// Stops the recording and returns the contents of `archive.jsonl` and
        /// `archive-1.jsonl` that exist, in order.
        async fn stop(self) -> Vec<String> {
            let _ = self.stop_sender.send(());
            self.recording.await.unwrap().unwrap();
            let mut archives = Vec::new();
            for file in ["archive.jsonl", "archive-1.jsonl"] {
                if let Ok(data) = tokio::fs::read_to_string(self.dir.join(file)).await {
                    archives.push(data);
                }
            }
            self.connection.close().await;
            tokio::fs::remove_dir_all(self.dir).await.unwrap();
            archives
        }
    }

    async fn publish_rendition_catalog(origin: &moqt::publish::Origin, heights: &[u32]) {
        let tracks = heights
            .iter()
            .map(|height| {
                let mut track =
                    msf::Track::new(&format!("video_{height}p"), KnownPackaging::Loc, true);
                track.role = Some(KnownTrackRole::Video.into());
                track.codec = Some("avc1.42C01E".into());
                track.height = Some(*height);
                track
            })
            .collect();
        let catalog = serde_json::to_vec(&Catalog::new(tracks, 0)).unwrap();
        origin
            .write(
                SWITCH_NAMESPACE,
                "catalog",
                OutgoingObject::plain(GroupBoundary::Next, Bytes::from(catalog)),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_catalog_that_swaps_the_recorded_rendition_starts_the_next_archive() {
        // Arrange
        let mut switch = RenditionSwitch::start().await;
        switch.write_frame("video_480p", "archive.jsonl").await;

        // Act: a taller rendition the recorder prefers
        publish_rendition_catalog(&switch.publisher.origin, &[480, 720]).await;
        switch.served("video_720p").await;
        switch.write_frame("video_720p", "archive-1.jsonl").await;
        let archives = switch.stop().await;

        // Assert: each archive holds one rendition and is complete
        let [first, next] = archives.as_slice() else {
            panic!("expected two archives, got {}", archives.len());
        };
        assert!(first.contains("\"track\":\"video_480p\""));
        assert!(!first.contains("\"track\":\"video_720p\""));
        assert!(next.contains("\"track\":\"video_720p\""));
        assert!(!next.contains("\"track\":\"video_480p\""));
        for data in [first, next] {
            assert!(data.lines().any(|line| line == "{\"complete\":true}"));
        }
    }

    #[tokio::test]
    async fn a_rendition_swapped_before_any_of_it_was_stored_keeps_the_archive() {
        // Arrange
        let mut switch = RenditionSwitch::start().await;

        // Act
        publish_rendition_catalog(&switch.publisher.origin, &[480, 720]).await;
        switch.served("video_720p").await;
        switch.write_frame("video_720p", "archive.jsonl").await;
        let archives = switch.stop().await;

        // Assert
        assert_eq!(archives.len(), 1);
        assert!(archives[0].contains("\"track\":\"video_720p\""));
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
                RecordingClock::started_at(0),
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
