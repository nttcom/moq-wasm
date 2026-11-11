use std::time::Duration;

use bytes::Bytes;
use gstreamer::{self as gst, prelude::*};
use mediapack::{
    MediaEvent, Timestamp, VideoSample,
    h264::annexb::{nal_units, with_start_codes},
    loc, mpegts,
};

use crate::{Rendition, TranscodedEvent, Transcoder, compositor::EventSource};

const FIXTURE_TS: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc.ts");
pub(crate) const FIXTURE_VP8_IVF: &[u8] =
    include_bytes!("../../mediapack/fixtures/testsrc_vp8.ivf");
pub(crate) const FIXTURE_VP9_IVF: &[u8] =
    include_bytes!("../../mediapack/fixtures/testsrc_vp9.ivf");
pub(crate) const FIXTURE_AV1_IVF: &[u8] =
    include_bytes!("../../mediapack/fixtures/testsrc_av1.ivf");
const FIXTURE_HEVC: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc_hevc.h265");
/// The VPS, SPS, PPS and slice of the keyframe `FIXTURE_HEVC` starts with.
const HEVC_KEYFRAME_NAL_UNITS: usize = 4;

/// H.264 + AAC events of `testsrc.ts` in demux order.
pub(crate) fn fixture_events() -> Vec<MediaEvent> {
    let mut demuxer = mpegts::Demuxer::new();
    let mut events = demuxer.push(FIXTURE_TS).unwrap();
    events.extend(demuxer.finish().unwrap());
    events
}

pub(crate) fn fixture_video_samples() -> Vec<VideoSample> {
    fixture_events()
        .into_iter()
        .filter_map(|event| match event {
            MediaEvent::Video(sample) => Some(sample),
            _ => None,
        })
        .collect()
}

pub(crate) async fn transcode_54p(samples: &[VideoSample]) -> Vec<TranscodedEvent> {
    let mut transcoder = Transcoder::new(&[Rendition {
        name: "54p".into(),
        width: 96,
        height: 54,
        bitrate_kbps: 100,
    }])
    .unwrap();
    let input = transcoder.input();
    for sample in samples {
        input.push(sample).unwrap();
    }
    input.finish().unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut outputs = Vec::new();
        while let Some(output) = transcoder.next().await {
            outputs.push(output.unwrap());
        }
        outputs
    })
    .await
    .expect("transcoder finished")
}

/// The frame payloads of an IVF file: a header whose length is at byte 6,
/// then frames each prefixed by a 4-byte size and an 8-byte timestamp.
fn ivf_frames(file: &[u8]) -> Vec<Bytes> {
    let mut position = u16::from_le_bytes([file[6], file[7]]) as usize;
    let mut frames = Vec::new();
    while position < file.len() {
        let size = u32::from_le_bytes(file[position..position + 4].try_into().unwrap()) as usize;
        position += 12;
        frames.push(Bytes::copy_from_slice(&file[position..position + size]));
        position += size;
    }
    frames
}

pub(crate) fn ivf_events(codec: loc::VideoCodec, fixture: &[u8]) -> Vec<MediaEvent> {
    loc_events(codec, ivf_frames(fixture))
}

/// The keyframe and the inter frame of `FIXTURE_HEVC`.
pub(crate) fn hevc_events() -> Vec<MediaEvent> {
    let nals: Vec<&[u8]> = nal_units(FIXTURE_HEVC).collect();
    let (keyframe, inter_frame) = nals.split_at(HEVC_KEYFRAME_NAL_UNITS);
    loc_events(
        loc::VideoCodec::H265,
        [keyframe, inter_frame]
            .map(|nals| with_start_codes(nals.iter().copied()))
            .to_vec(),
    )
}

/// Timestamps the frames 30 fps apart.
fn loc_events(codec: loc::VideoCodec, payloads: Vec<Bytes>) -> Vec<MediaEvent> {
    let mut demuxer = loc::Demuxer::video(codec);
    let mut events = Vec::new();
    for (index, payload) in payloads.into_iter().enumerate() {
        let object = loc::LocObject {
            extensions: vec![loc::LocExtension::varint(
                loc::TIMESTAMP_ID,
                index as u64 * 33_333,
            )],
            payload,
        };
        events.extend(demuxer.push(&object).unwrap());
    }
    events
}

/// VideoToolbox, which decodebin prefers on macOS, now and then drops the
/// last frame when it is drained at end of stream.
pub(crate) fn decode_in_software() {
    gst::init().unwrap();
    let registry = gst::Registry::get();
    for name in ["vtdec_hw", "vtdec"] {
        if let Some(decoder) = registry.lookup_feature(name) {
            decoder.set_rank(gst::Rank::NONE);
        }
    }
}

pub(crate) fn source(events: Vec<MediaEvent>) -> Option<EventSource> {
    Some(Box::new(events.into_iter().map(Ok)))
}

pub(crate) fn audio_events() -> Vec<MediaEvent> {
    fixture_events()
        .into_iter()
        .filter(|event| matches!(event, MediaEvent::AudioConfig(_) | MediaEvent::Audio(_)))
        .collect()
}

pub(crate) fn shifted(events: Vec<MediaEvent>, shift: Timestamp) -> Vec<MediaEvent> {
    events
        .into_iter()
        .map(|event| match event {
            MediaEvent::Video(mut sample) => {
                sample.pts = sample.pts.saturating_add(shift);
                sample.dts = sample.dts.saturating_add(shift);
                MediaEvent::Video(sample)
            }
            MediaEvent::Audio(mut sample) => {
                sample.pts = sample.pts.saturating_add(shift);
                MediaEvent::Audio(sample)
            }
            other => other,
        })
        .collect()
}

pub(crate) fn ms(millis: u64) -> gst::ClockTime {
    gst::ClockTime::from_mseconds(millis)
}
