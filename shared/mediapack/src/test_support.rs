use bytes::Bytes;

use crate::{
    aac::{AudioSpecificConfig, adts::HEADER_LENGTH},
    h264::{AvcDecoderConfigurationRecord, annexb::with_start_codes},
    mpegts::parser::{PACKET_SIZE, SYNC_BYTE},
    sample::{AudioSample, MediaEvent, VideoSample},
};

pub(crate) const FIXTURE_TS: &[u8] = include_bytes!("../fixtures/testsrc.ts");
pub(crate) const FIXTURE_FLV: &[u8] = include_bytes!("../fixtures/testsrc.flv");

pub(crate) const FIXTURE_SPS: [u8; 24] = [
    0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00,
    0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
];
pub(crate) const FIXTURE_PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
pub(crate) const IDR_SLICE: [u8; 3] = [0x65, 0x88, 0x84];
pub(crate) const NON_IDR_SLICE: [u8; 3] = [0x41, 0x9a, 0x22];

pub(crate) fn fixture_record() -> AvcDecoderConfigurationRecord {
    AvcDecoderConfigurationRecord::from_parameter_sets(
        vec![Bytes::from_static(&FIXTURE_SPS)],
        vec![Bytes::from_static(&FIXTURE_PPS)],
    )
    .unwrap()
}

pub(crate) fn keyframe_annexb() -> Bytes {
    with_start_codes([&FIXTURE_SPS[..], &FIXTURE_PPS, &IDR_SLICE])
}

pub(crate) fn delta_frame_annexb() -> Bytes {
    with_start_codes([&NON_IDR_SLICE[..]])
}

pub(crate) fn mono_48k() -> AudioSpecificConfig {
    AudioSpecificConfig::new(2, 48_000, 1)
}

pub(crate) fn adts_frame(payload: &[u8]) -> Vec<u8> {
    let frame_length = payload.len() + HEADER_LENGTH;
    let mut frame = vec![
        0xFF,
        0xF1,
        0x4C,
        0x40 | (frame_length >> 11) as u8 & 0x03,
        (frame_length >> 3) as u8,
        ((frame_length as u8 & 0x07) << 5) | 0x1F,
        0xFC,
    ];
    frame.extend_from_slice(payload);
    frame
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

pub(crate) fn count_events(events: &[MediaEvent], matches: impl Fn(&MediaEvent) -> bool) -> usize {
    events.iter().filter(|event| matches(event)).count()
}

pub(crate) fn ts_packet(
    pid: u16,
    payload_unit_start: bool,
    counter: u8,
    payload: &[u8],
) -> Vec<u8> {
    assert!(payload.len() <= PACKET_SIZE - 4);
    let stuffing = PACKET_SIZE - 4 - payload.len();
    let adaptation_field_control = if stuffing == 0 { 0b01 } else { 0b11 };
    let mut packet = vec![
        SYNC_BYTE,
        (payload_unit_start as u8) << 6 | (pid >> 8) as u8,
        pid as u8,
        adaptation_field_control << 4 | counter,
    ];
    if stuffing > 0 {
        let adaptation_length = stuffing - 1;
        packet.push(adaptation_length as u8);
        if adaptation_length > 0 {
            packet.push(0x00);
            packet.extend(std::iter::repeat_n(0xFF, adaptation_length - 1));
        }
    }
    packet.extend_from_slice(payload);
    packet
}

pub(crate) fn ts_packets(pid: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (index, chunk) in payload.chunks(PACKET_SIZE - 4).enumerate() {
        out.extend(ts_packet(pid, index == 0, (index % 16) as u8, chunk));
    }
    out
}

fn psi_section(table_id: u8, body: &[u8]) -> Vec<u8> {
    let section_length = 5 + body.len() + 4;
    let mut section = vec![0x00];
    section.push(table_id);
    section.push(0xB0 | (section_length >> 8) as u8);
    section.push(section_length as u8);
    section.extend_from_slice(&[0x00, 0x01, 0xC1, 0x00, 0x00]);
    section.extend_from_slice(body);
    section.extend_from_slice(&[0, 0, 0, 0]);
    section
}

pub(crate) fn pat_section(pmt_pid: u16) -> Vec<u8> {
    psi_section(
        0x00,
        &[0x00, 0x01, 0xE0 | (pmt_pid >> 8) as u8, pmt_pid as u8],
    )
}

pub(crate) fn pmt_section(streams: &[(u16, u8)]) -> Vec<u8> {
    let mut body = vec![0xE1, 0x00, 0xF0, 0x00];
    for (pid, stream_type) in streams {
        body.push(*stream_type);
        body.push(0xE0 | (pid >> 8) as u8);
        body.push(*pid as u8);
        body.extend_from_slice(&[0xF0, 0x00]);
    }
    psi_section(0x02, &body)
}

fn pes_timestamp(prefix: u8, ticks: u64) -> [u8; 5] {
    [
        prefix << 4 | ((ticks >> 29) as u8 & 0x0E) | 1,
        (ticks >> 22) as u8,
        ((ticks >> 14) as u8 & 0xFE) | 1,
        (ticks >> 7) as u8,
        ((ticks << 1) as u8) | 1,
    ]
}

pub(crate) fn pes_packet(stream_id: u8, pts: u64, dts: Option<u64>, payload: &[u8]) -> Vec<u8> {
    let mut header_data = pes_timestamp(if dts.is_some() { 0x3 } else { 0x2 }, pts).to_vec();
    if let Some(dts) = dts {
        header_data.extend(pes_timestamp(0x1, dts));
    }
    let mut pes = vec![0x00, 0x00, 0x01, stream_id, 0x00, 0x00, 0x80];
    pes.push(if dts.is_some() { 0xC0 } else { 0x80 });
    pes.push(header_data.len() as u8);
    pes.extend(header_data);
    pes.extend_from_slice(payload);
    pes
}
