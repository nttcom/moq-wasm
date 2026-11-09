use std::sync::atomic::{AtomicU64, Ordering};

use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, VideoSample,
    loc::{self, LocObject},
    mp4, mpegts,
};
use moqt::{
    Endpoint, NATIVE, SessionEvent, TrackObject,
    publish::Origin,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
};
use tokio::sync::mpsc;

use object_store::{ObjectStoreExt, PutPayload};

use crate::{
    LauncherArgs, RoomArgs, ServeArgs,
    archive_chunks::ChunkWriter,
    archive_reader::StoredRecording,
    archive_receiver::Event,
    launcher::{Launcher, LauncherKind},
    pipeline::capture_timestamp,
    recordings::Recordings,
    store::RecordingStore,
};

const FIXTURE_TS: &[u8] = include_bytes!("../../../crates/mediapack/fixtures/testsrc.ts");
const CAPTURE_ORIGIN: Timestamp = Timestamp::from_micros(1_700_000_000_000_000);

/// The fixture's media events as LOC track objects, grouped per keyframe
/// the way `broadcast` numbers them, plus the AAC config the catalog
/// would carry.
pub(crate) struct Fixture {
    pub(crate) events: Vec<MediaEvent>,
    pub(crate) video: Vec<TrackObject>,
    pub(crate) audio: Vec<TrackObject>,
    pub(crate) audio_config: AudioConfig,
}

pub(crate) fn fixture() -> Fixture {
    let mut demuxer = mpegts::Demuxer::new();
    let mut events = demuxer.push(FIXTURE_TS).unwrap();
    events.extend(demuxer.finish().unwrap());
    let muxer = loc::Muxer::new(CAPTURE_ORIGIN);
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    let mut group_id = 0;
    for event in &events {
        let Some(object) = muxer.push(event) else {
            continue;
        };
        match event {
            MediaEvent::Video(sample) => {
                if sample.is_keyframe {
                    group_id += 1;
                }
                video.push(track_object(group_id, video.len() as u64, object));
            }
            MediaEvent::Audio(_) => audio.push(track_object(group_id, audio.len() as u64, object)),
            _ => {}
        }
    }
    let audio_config = events
        .iter()
        .find_map(|event| match event {
            MediaEvent::AudioConfig(config) => Some(config.clone()),
            _ => None,
        })
        .unwrap();
    Fixture {
        events,
        video,
        audio,
        audio_config,
    }
}

pub(crate) fn track_object(group_id: u64, object_id: u64, object: LocObject) -> TrackObject {
    TrackObject {
        group_id,
        forwarding: moqt::object::ForwardingPreference::Subgroup { subgroup_id: 0 },
        object_id,
        extension_headers: loc::to_extension_headers(&object.extensions),
        payload: object.payload,
    }
}

pub(crate) fn demux(data: &[u8]) -> Vec<MediaEvent> {
    let mut demuxer = mp4::Demuxer::new();
    let mut events = demuxer.push(data).unwrap();
    events.extend(demuxer.finish().unwrap());
    events
}

pub(crate) fn video_samples(events: &[MediaEvent]) -> Vec<&VideoSample> {
    events
        .iter()
        .filter_map(|event| match event {
            MediaEvent::Video(sample) => Some(sample),
            _ => None,
        })
        .collect()
}

pub(crate) fn audio_samples(events: &[MediaEvent]) -> Vec<&AudioSample> {
    events
        .iter()
        .filter_map(|event| match event {
            MediaEvent::Audio(sample) => Some(sample),
            _ => None,
        })
        .collect()
}

pub(crate) fn first_video_capture(object: &TrackObject) -> u64 {
    capture_timestamp(object).unwrap().micros()
}

pub(crate) fn opus_object(group_id: u64, object_id: u64, capture_micros: u64) -> TrackObject {
    track_object(
        group_id,
        object_id,
        LocObject {
            extensions: vec![loc::LocExtension::varint(loc::TIMESTAMP_ID, capture_micros)],
            payload: bytes::Bytes::from_static(&OPUS_20_MS_FRAME),
        },
    )
}

/// TOC configuration 31, code 0: one 20 ms CELT frame.
pub(crate) const OPUS_20_MS_FRAME: [u8; 4] = [0xFC, 0x01, 0x02, 0x03];

pub(crate) fn opus_config() -> AudioConfig {
    AudioConfig::Opus {
        sample_rate: 48_000,
        channels: 2,
    }
}

/// A new empty directory under the system temp directory. The counter keeps
/// tests running in parallel apart: the clock does not tick every nanosecond.
pub(crate) fn scratch_dir() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let count = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "recorder-test-{}-{nanos}-{count}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `testsrc.ts` (H.264 + AAC from 1.4 s) as a recorded fragmented MP4 at
/// `path`.
pub(crate) fn write_fixture_fmp4(path: &std::path::Path) {
    let mut output = Vec::new();
    mediapack::transmux(
        mediapack::InputFormat::MpegTs,
        mediapack::OutputFormat::Fmp4,
        &mut &FIXTURE_TS[..],
        &mut output,
    )
    .unwrap();
    std::fs::write(path, output).unwrap();
}

/// The objects of each track of one archive, written in the order given.
pub(crate) type StoredArchive<'a> = Vec<(&'a str, Vec<moqt::object::Object>)>;

pub(crate) async fn stored_recording(
    tracks: StoredArchive<'_>,
) -> (RecordingStore, StoredRecording) {
    stored_room(vec![("anon/room/alice", vec![tracks])]).await
}

/// A MoQ room recording `id` of `anon/room` in an in-memory store. Each of
/// `members` is a namespace under `anon/room` with one camera archive per time
/// it joined.
pub(crate) async fn stored_room(
    members: Vec<(&str, Vec<StoredArchive<'_>>)>,
) -> (RecordingStore, StoredRecording) {
    let store = RecordingStore::open(&url::Url::parse("memory:///").unwrap()).unwrap();
    let prefix = store.recording_prefix("id");
    let mut listed = Vec::new();
    for (member_index, (namespace, archives)) in members.into_iter().enumerate() {
        let mut keys = Vec::new();
        for (archive_index, tracks) in archives.into_iter().enumerate() {
            let mut writer = ChunkWriter::new(
                store.object_store(),
                prefix
                    .clone()
                    .join("archive")
                    .join(format!("{member_index}-{archive_index}")),
                namespace.into(),
            );
            for (name, objects) in tracks {
                for object in objects {
                    writer.push(name, &Event::Object(object)).await.unwrap();
                }
            }
            keys.push(writer.finish().await.unwrap().to_string());
        }
        listed.push(
            serde_json::json!({ "namespace": namespace, "camera": keys, "screen_share": [] }),
        );
    }
    let manifest = serde_json::json!({
        "version": 2, "format": "moq", "started_at_ms": "1700000000000", "members": listed,
    });
    store
        .object_store()
        .put(
            &prefix.join("manifest.json"),
            PutPayload::from(manifest.to_string()),
        )
        .await
        .unwrap();
    let recording = StoredRecording::open(&store, "id", "anon/room")
        .await
        .unwrap();
    (store, recording)
}

/// A Normal object on subgroup 0.
pub(crate) fn stored_object(
    group_id: u64,
    object_id: u64,
    payload: impl Into<bytes::Bytes>,
) -> moqt::object::Object {
    moqt::object::Object {
        location: moqt::object::Location {
            group_id,
            object_id,
        },
        forwarding: moqt::object::ForwardingPreference::Subgroup { subgroup_id: 0 },
        publisher_priority: 0,
        status: moqt::object::ObjectStatus::Normal,
        extension_headers: moqt::object::ExtensionHeaders::default(),
        payload: payload.into(),
        ends_group: false,
    }
}

/// Object 0 of group `group_id`, as `stored_object` builds it, as a room
/// recording stores it: its Timestamp `timestamp_us` is media time since the
/// recording started (`RecordingClock`).
pub(crate) fn recorded_object(group_id: u64, timestamp_us: u64) -> moqt::object::Object {
    use mediapack::loc::{LocExtension, TIMESCALE_ID, TIMESTAMP_ID, to_extension_headers};

    let mut object = stored_object(group_id, 0, "frame");
    object.extension_headers = to_extension_headers(&[
        LocExtension::varint(TIMESTAMP_ID, timestamp_us),
        LocExtension::varint(TIMESCALE_ID, 1_000_000),
    ]);
    object
}

/// A camera archive as `stored_recording` writes it: one catalog object
/// listing `video` and `video_timeline`, and one video group per second from
/// group 10, whose Timestamps count from 0 as a recording's do.
pub(crate) async fn recorded_camera() -> StoredRecording {
    use msf::{Catalog, KnownPackaging, KnownTrackRole, Track};

    let mut video = Track::new("video", KnownPackaging::Loc, true);
    video.role = Some(KnownTrackRole::Video.into());
    video.codec = Some("avc1.42C01E".to_string());
    video.render_group = Some(1);
    video.target_latency = Some(500);
    let mut timeline = Track::new("video_timeline", KnownPackaging::MediaTimeline, true);
    timeline.depends = Some(vec!["video".to_string(), "video_480p".to_string()]);
    let catalog = serde_json::to_vec(&Catalog::new(vec![video, timeline], 1)).unwrap();
    let frames = (10..13)
        .map(|group| recorded_object(group, (group - 10) * 1_000_000))
        .collect();
    let (_store, recording) = stored_recording(vec![
        ("catalog", vec![stored_object(0, 0, catalog)]),
        ("video", frames),
    ])
    .await;
    recording
}

/// A meeting's recording: `anon/room/alice` joined twice with `video` and
/// `audio`, over seconds 0 to 2 and 10 to 12 of the recording; `anon/room/bob`
/// once with `video` alone over the same seconds as alice's first session;
/// and the transcriber, whose `transcript` holds one object per document in
/// one group.
pub(crate) async fn recorded_meeting(documents: Vec<serde_json::Value>) -> StoredRecording {
    use msf::{Catalog, KnownPackaging, KnownTrackRole, Track};

    let session = |start_s: u64, with_audio: bool| {
        let mut tracks = vec![("video", KnownTrackRole::Video)];
        if with_audio {
            tracks.push(("audio", KnownTrackRole::Audio));
        }
        let listed = tracks
            .iter()
            .map(|(name, role)| {
                let mut track = Track::new(name, KnownPackaging::Loc, true);
                track.role = Some(role.clone().into());
                track.render_group = Some(1);
                track
            })
            .collect();
        let catalog = serde_json::to_vec(&Catalog::new(listed, 1)).unwrap();
        let mut archive = vec![("catalog", vec![stored_object(0, 0, catalog)])];
        for (name, _) in tracks {
            let objects = (0..3)
                .map(|group| recorded_object(group, (start_s + group) * 1_000_000))
                .collect();
            archive.push((name, objects));
        }
        archive
    };
    let transcript = documents
        .iter()
        .enumerate()
        .map(|(object_id, records)| {
            stored_object(7, object_id as u64, serde_json::to_vec(records).unwrap())
        })
        .collect();
    let (_store, recording) = stored_room(vec![
        ("anon/room/alice", vec![session(0, true), session(10, true)]),
        ("anon/room/bob", vec![session(0, false)]),
        (
            "anon/room/bots/transcriber",
            vec![vec![("transcript", transcript)]],
        ),
    ])
    .await;
    recording
}

pub(crate) fn transcript_record(m_ms: u64, speaker: &str) -> serde_json::Value {
    serde_json::json!({ "m": m_ms, "data": { "speaker": speaker, "text": format!("{speaker} at {m_ms}") } })
}

pub(crate) const TOKEN_MINT_URL: &str = "http://token-mint.invalid";

/// A recording API over an in-memory store, whose jobs exit at once, that
/// records temporary apps' rooms with `TOKEN_MINT_URL`.
pub(crate) fn recordings(max_recordings: usize, composite: bool) -> Recordings {
    let store_url = url::Url::parse("memory:///").unwrap();
    let config = ServeArgs {
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        namespace_root: "anon".to_string(),
        auth_vts_url: None,
        max_recordings,
        composite,
        token_mint_url: Some(TOKEN_MINT_URL.to_string()),
        store: store_url.clone(),
        output_dir: std::env::temp_dir(),
        room: RoomArgs {
            relay: "moqt://127.0.0.1:1".to_string(),
            segment_seconds: None,
            max_recording_seconds: 7200,
            empty_room_seconds: 300,
            max_video_height: 720,
            insecure_skip_tls_verify: false,
        },
        launcher: LauncherArgs {
            launcher: LauncherKind::Local,
            record_job: None,
            composite_job: None,
        },
    };
    let launcher = Launcher::Local {
        program: std::path::PathBuf::from("/usr/bin/true"),
    };
    Recordings::new(config, RecordingStore::open(&store_url).unwrap(), launcher)
}

/// A publisher that serves the tracks registered with `origin` to the one
/// session that connects to `url`, as a relay would.
pub(crate) struct FakePublisher {
    pub(crate) url: String,
    pub(crate) origin: Origin,
    /// The track name of each SUBSCRIBE, sent once `origin` serves it.
    pub(crate) served: mpsc::UnboundedReceiver<String>,
}

pub(crate) struct ConnectedSessions {
    pub(crate) client: crate::relay::RelayConnection,
    pub(crate) server: std::sync::Arc<moqt::Session>,
    _endpoint: Endpoint<NATIVE>,
}

pub(crate) async fn connected_sessions() -> ConnectedSessions {
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&scratch_dir(), &["localhost", "127.0.0.1"]).unwrap();
    let endpoint = Endpoint::<NATIVE>::create_server(&ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
    })
    .unwrap();
    let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
    let (client, server) = tokio::join!(
        crate::relay::RelayConnection::connect(&url, false, None),
        async {
            endpoint
                .accept()
                .await
                .unwrap()
                .await
                .unwrap()
                .accept(&tracing::Span::current())
                .await
                .unwrap()
        }
    );
    ConnectedSessions {
        client: client.unwrap(),
        server: std::sync::Arc::new(server),
        _endpoint: endpoint,
    }
}

impl FakePublisher {
    pub(crate) fn spawn() -> Self {
        let CertificateFiles {
            cert_path,
            key_path,
        } = write_self_signed_cert(&scratch_dir(), &["localhost", "127.0.0.1"]).unwrap();
        let endpoint = Endpoint::<NATIVE>::create_server(&ServerConfig {
            port: 0,
            cert_path,
            key_path,
            keep_alive_interval_sec: 5,
        })
        .unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let origin = Origin::default();
        let (served_sender, served) = mpsc::unbounded_channel();
        let serving = origin.clone();
        tokio::spawn(async move {
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = handshake.accept(&tracing::Span::current()).await.unwrap();
            while let Ok(event) = session.receive_event().await {
                let subscribed = match &event {
                    SessionEvent::Subscribe(handler) => Some(handler.track_name.clone()),
                    _ => None,
                };
                serving.handle(event).await;
                if let Some(track_name) = subscribed {
                    let _ = served_sender.send(track_name);
                }
            }
        });
        Self {
            url,
            origin,
            served,
        }
    }
}

/// A publisher that sends the supplied datagrams after accepting a SUBSCRIBE.
pub(crate) fn datagram_publisher(objects: Vec<moqt::object::Object>) -> String {
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&scratch_dir(), &["localhost", "127.0.0.1"]).unwrap();
    let endpoint = Endpoint::<NATIVE>::create_server(&ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
    })
    .unwrap();
    let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
    tokio::spawn(async move {
        let session = endpoint
            .accept()
            .await
            .unwrap()
            .await
            .unwrap()
            .accept(&tracing::Span::current())
            .await
            .unwrap();
        while let Ok(event) = session.receive_event().await {
            if let SessionEvent::Subscribe(handler) = event {
                let alias = handler
                    .ok(0, moqt::object::ContentExists::False)
                    .await
                    .unwrap();
                let subscription = handler.into_subscription(alias);
                let mut sender = session.publisher().create_datagram(&subscription);
                for object in &objects {
                    sender.send_object(object).await.unwrap();
                }
                let _ = session.closed().await;
                break;
            }
        }
    });
    url
}

/// A publisher that, after accepting a SUBSCRIBE, sends `reset` on a subgroup
/// stream, and once the returned sender fires resets that stream and sends
/// `object` on a subgroup stream it closes.
pub(crate) fn reset_stream_publisher(
    reset: moqt::object::Object,
    object: moqt::object::Object,
) -> (String, tokio::sync::oneshot::Sender<()>) {
    let (proceed_sender, proceed_receiver) = tokio::sync::oneshot::channel();
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&scratch_dir(), &["localhost", "127.0.0.1"]).unwrap();
    let endpoint = Endpoint::<NATIVE>::create_server(&ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
    })
    .unwrap();
    let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
    tokio::spawn(async move {
        let session = endpoint
            .accept()
            .await
            .unwrap()
            .await
            .unwrap()
            .accept(&tracing::Span::current())
            .await
            .unwrap();
        while let Ok(event) = session.receive_event().await {
            if let SessionEvent::Subscribe(handler) = event {
                let alias = handler
                    .ok(0, moqt::object::ContentExists::False)
                    .await
                    .unwrap();
                let subscription = handler.into_subscription(alias);
                let factory = session.publisher().create_stream(&subscription);
                let start = |object: &moqt::object::Object| moqt::object::SubgroupStart {
                    group_id: object.location.group_id,
                    subgroup_id: 0,
                    publisher_priority: object.publisher_priority,
                    ends_group: false,
                };
                let mut first = factory
                    .next()
                    .await
                    .unwrap()
                    .start(start(&reset))
                    .await
                    .unwrap();
                first.send_object(&reset).await.unwrap();
                let _ = proceed_receiver.await;
                first
                    .reset(moqt::object::DataStreamResetCode::Cancelled)
                    .await
                    .unwrap();
                let mut second = factory
                    .next()
                    .await
                    .unwrap()
                    .start(start(&object))
                    .await
                    .unwrap();
                second.send_object(&object).await.unwrap();
                second.close().await.unwrap();
                let _ = session.closed().await;
                break;
            }
        }
    });
    (url, proceed_sender)
}
