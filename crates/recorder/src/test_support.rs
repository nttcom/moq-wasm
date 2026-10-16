use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, VideoSample,
    loc::{self, LocObject},
    mp4, mpegts,
};
use moqt::TrackObject;

use crate::pipeline::capture_timestamp;

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
            extensions: vec![loc::LocExtension::varint(
                loc::CAPTURE_TIMESTAMP_ID,
                capture_micros,
            )],
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

/// A new empty directory under the system temp directory.
pub(crate) fn scratch_dir() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("recorder-test-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
