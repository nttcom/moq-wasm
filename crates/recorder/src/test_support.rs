use std::sync::atomic::{AtomicU64, Ordering};

use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, VideoSample,
    loc::{self, LocObject},
    mp4, mpegts,
};
use moqt::{
    DUAL, Endpoint, SessionEvent, TrackObject,
    publish::Origin,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
};
use tokio::sync::mpsc;

use crate::{
    LauncherArgs, RoomArgs,
    launcher::{Launcher, LauncherKind},
    pipeline::capture_timestamp,
    recordings::{Recordings, ServeConfig},
    store::RecordingStore,
};

const FIXTURE_TS: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc.ts");
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
        subgroup_id: 0,
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

/// A recording API over an in-memory store, whose jobs exit at once.
pub(crate) fn recordings(max_recordings: usize, composite: bool) -> Recordings {
    let store_url = url::Url::parse("memory:///").unwrap();
    let config = ServeConfig {
        namespace_root: "anon".to_string(),
        max_recordings,
        composite,
        store_url: store_url.clone(),
        output_dir: std::env::temp_dir(),
        room: RoomArgs {
            relay: "moqt://127.0.0.1:1".to_string(),
            segment_seconds: None,
            max_recording_seconds: 7200,
            empty_room_seconds: 300,
            max_video_height: 480,
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

impl FakePublisher {
    pub(crate) fn spawn() -> Self {
        let CertificateFiles {
            cert_path,
            key_path,
        } = write_self_signed_cert(&scratch_dir(), &["localhost", "127.0.0.1"]).unwrap();
        let endpoint = Endpoint::<DUAL>::create_server(&ServerConfig {
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
            let session = handshake.accept().await.unwrap();
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
