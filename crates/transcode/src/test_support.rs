use bytes::Bytes;
use gstreamer::{self as gst, prelude::*};
use mediapack::{MediaEvent, mpegts};

const FIXTURE_TS: &[u8] = include_bytes!("../../mediapack/fixtures/testsrc.ts");
pub(crate) const FIXTURE_VP8_IVF: &[u8] =
    include_bytes!("../../mediapack/fixtures/testsrc_vp8.ivf");
pub(crate) const FIXTURE_VP9_IVF: &[u8] =
    include_bytes!("../../mediapack/fixtures/testsrc_vp9.ivf");
pub(crate) const FIXTURE_AV1_IVF: &[u8] =
    include_bytes!("../../mediapack/fixtures/testsrc_av1.ivf");

/// H.264 + AAC events of `testsrc.ts` in demux order.
pub(crate) fn fixture_events() -> Vec<MediaEvent> {
    let mut demuxer = mpegts::Demuxer::new();
    let mut events = demuxer.push(FIXTURE_TS).unwrap();
    events.extend(demuxer.finish().unwrap());
    events
}

/// The frame payloads of an IVF file: a header whose length is at byte 6,
/// then frames each prefixed by a 4-byte size and an 8-byte timestamp.
pub(crate) fn ivf_frames(file: &[u8]) -> Vec<Bytes> {
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
