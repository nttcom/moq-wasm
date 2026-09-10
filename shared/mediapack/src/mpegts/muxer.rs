use anyhow::{Context, Result};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{
    aac::{AudioSpecificConfig, adts},
    mpegts::parser::{
        CLOCK_RATE, PACKET_SIZE, PAT_PID, STREAM_TYPE_AAC_ADTS, STREAM_TYPE_H264, SYNC_BYTE,
    },
    sample::{AudioSample, MediaEvent, StreamSet, Timestamp, VideoSample},
};

const PMT_PID: u16 = 0x1000;
const VIDEO_PID: u16 = 0x0100;
const AUDIO_PID: u16 = 0x0101;
const PROGRAM_NUMBER: u16 = 1;
const VIDEO_STREAM_ID: u8 = 0xE0;
const AUDIO_STREAM_ID: u8 = 0xC0;
const SECTION_CRC_LENGTH: usize = 4;
const PES_HEADER_PREFIX_LENGTH: usize = 6;
const TIMESTAMP_LENGTH: usize = 5;
const MAX_PES_PACKET_LENGTH: usize = u16::MAX as usize;

pub struct Muxer {
    streams: StreamSet,
    audio_config: Option<AudioSpecificConfig>,
    continuity: [u8; 4],
    program_tables_pending: bool,
}

impl Default for Muxer {
    fn default() -> Self {
        Self {
            streams: StreamSet {
                has_video: true,
                has_audio: true,
            },
            audio_config: None,
            continuity: [0; 4],
            program_tables_pending: true,
        }
    }
}

impl Muxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: &MediaEvent) -> Result<Bytes> {
        match event {
            MediaEvent::Streams(streams) => {
                self.streams = *streams;
                Ok(Bytes::new())
            }
            MediaEvent::AudioConfig(config) => {
                self.audio_config = Some(config.clone());
                Ok(Bytes::new())
            }
            MediaEvent::VideoConfig(_) => Ok(Bytes::new()),
            MediaEvent::Video(sample) => self.write_video(sample),
            MediaEvent::Audio(sample) => self.write_audio(sample),
        }
    }

    fn write_video(&mut self, sample: &VideoSample) -> Result<Bytes> {
        let pes = pes_packet(
            VIDEO_STREAM_ID,
            PesTimestamps {
                pts: sample.pts,
                dts: Some(sample.dts),
            },
            &sample.data,
        );
        let mut out = BytesMut::new();
        if self.program_tables_pending || sample.is_keyframe {
            out.put_slice(&self.program_tables());
        }
        out.put_slice(&self.packetize(VIDEO_PID, &pes, Some(sample.dts)));
        Ok(out.freeze())
    }

    fn write_audio(&mut self, sample: &AudioSample) -> Result<Bytes> {
        let config = self
            .audio_config
            .clone()
            .context("audio sample muxed before its AudioSpecificConfig")?;
        let pes = pes_packet(
            AUDIO_STREAM_ID,
            PesTimestamps {
                pts: sample.pts,
                dts: None,
            },
            &adts::frame(&config, &sample.data)?,
        );
        let mut out = BytesMut::new();
        if self.program_tables_pending || !self.streams.has_video {
            out.put_slice(&self.program_tables());
        }
        let pcr = (!self.streams.has_video).then_some(sample.pts);
        out.put_slice(&self.packetize(AUDIO_PID, &pes, pcr));
        Ok(out.freeze())
    }

    fn program_tables(&mut self) -> Bytes {
        self.program_tables_pending = false;
        let mut out = BytesMut::new();
        out.put_slice(&self.packetize(PAT_PID, &pat_section(), None));
        out.put_slice(&self.packetize(PMT_PID, &self.pmt_section(), None));
        out.freeze()
    }

    fn pmt_section(&self) -> Bytes {
        let mut streams = Vec::new();
        if self.streams.has_video {
            streams.push((VIDEO_PID, STREAM_TYPE_H264));
        }
        if self.streams.has_audio {
            streams.push((AUDIO_PID, STREAM_TYPE_AAC_ADTS));
        }
        let pcr_pid = if self.streams.has_video {
            VIDEO_PID
        } else {
            AUDIO_PID
        };

        let mut body = BytesMut::new();
        body.put_u16(0xE000 | pcr_pid);
        body.put_u16(0xF000);
        for (pid, stream_type) in streams {
            body.put_u8(stream_type);
            body.put_u16(0xE000 | pid);
            body.put_u16(0xF000);
        }
        psi_section(0x02, PROGRAM_NUMBER, &body)
    }

    fn packetize(&mut self, pid: u16, payload: &[u8], pcr: Option<Timestamp>) -> Bytes {
        let mut out = BytesMut::new();
        let mut offset = 0;
        let mut payload_unit_start = true;
        while offset < payload.len() {
            let adaptation = self.adaptation_field(payload.len() - offset, payload_unit_start, pcr);
            let space = PACKET_SIZE - 4 - adaptation.len();
            let chunk = &payload[offset..(offset + space).min(payload.len())];
            out.put_u8(SYNC_BYTE);
            out.put_u8((payload_unit_start as u8) << 6 | (pid >> 8) as u8);
            out.put_u8(pid as u8);
            let adaptation_control = if adaptation.is_empty() { 0b01 } else { 0b11 };
            out.put_u8(adaptation_control << 4 | self.next_continuity(pid));
            out.put_slice(&adaptation);
            out.put_slice(chunk);
            offset += chunk.len();
            payload_unit_start = false;
        }
        out.freeze()
    }

    fn adaptation_field(
        &self,
        remaining: usize,
        payload_unit_start: bool,
        pcr: Option<Timestamp>,
    ) -> Bytes {
        let clock = pcr.filter(|_| payload_unit_start);
        let mut field = BytesMut::new();
        if let Some(clock) = clock {
            field.put_u8(0x10);
            field.put_slice(&program_clock_reference(clock));
        }

        let minimum_length = if clock.is_some() { 8 } else { 0 };
        let adaptation_length = minimum_length.max((PACKET_SIZE - 4).saturating_sub(remaining));
        if adaptation_length == 0 {
            return Bytes::new();
        }
        if adaptation_length == 1 {
            return Bytes::from_static(&[0]);
        }
        if field.is_empty() {
            field.put_u8(0x00);
        }
        field.resize(adaptation_length - 1, 0xFF);
        let mut out = BytesMut::with_capacity(field.len() + 1);
        out.put_u8(field.len() as u8);
        out.put_slice(&field);
        out.freeze()
    }

    fn next_continuity(&mut self, pid: u16) -> u8 {
        let index = match pid {
            PAT_PID => 0,
            PMT_PID => 1,
            VIDEO_PID => 2,
            _ => 3,
        };
        let counter = self.continuity[index];
        self.continuity[index] = (counter + 1) & 0x0F;
        counter
    }
}

fn pat_section() -> Bytes {
    let mut body = BytesMut::new();
    body.put_u16(PROGRAM_NUMBER);
    body.put_u16(0xE000 | PMT_PID);
    psi_section(0x00, 0, &body)
}

fn psi_section(table_id: u8, table_id_extension: u16, body: &[u8]) -> Bytes {
    let section_length = 5 + body.len() + SECTION_CRC_LENGTH;
    let mut section = BytesMut::new();
    section.put_u8(table_id);
    section.put_u16(0xB000 | section_length as u16);
    section.put_u16(table_id_extension);
    section.put_u8(0xC1);
    section.put_u8(0x00);
    section.put_u8(0x00);
    section.put_slice(body);
    let crc = crc32_mpeg(&section);

    let mut out = BytesMut::with_capacity(1 + section.len() + SECTION_CRC_LENGTH);
    out.put_u8(0x00);
    out.put_slice(&section);
    out.put_u32(crc);
    out.freeze()
}

fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in data {
        crc ^= (*byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

struct PesTimestamps {
    pts: Timestamp,
    dts: Option<Timestamp>,
}

fn pes_packet(stream_id: u8, timestamps: PesTimestamps, payload: &[u8]) -> Bytes {
    let mut header_data = BytesMut::new();
    let pts = timestamps.pts;
    let flags = match timestamps.dts {
        Some(dts) => {
            header_data.put_slice(&timestamp(0b0011, pts));
            header_data.put_slice(&timestamp(0b0001, dts));
            0xC0
        }
        None => {
            header_data.put_slice(&timestamp(0b0010, pts));
            0x80
        }
    };

    let packet_length = 3 + header_data.len() + payload.len();
    let mut out = BytesMut::with_capacity(PES_HEADER_PREFIX_LENGTH + packet_length);
    out.put_slice(&[0x00, 0x00, 0x01, stream_id]);
    out.put_u16(if packet_length <= MAX_PES_PACKET_LENGTH {
        packet_length as u16
    } else {
        0
    });
    out.put_u8(0x80);
    out.put_u8(flags);
    out.put_u8(header_data.len() as u8);
    out.put_slice(&header_data);
    out.put_slice(payload);
    out.freeze()
}

fn timestamp(prefix: u8, value: Timestamp) -> [u8; TIMESTAMP_LENGTH] {
    let ticks = value.ticks(CLOCK_RATE);
    [
        prefix << 4 | ((ticks >> 29) as u8 & 0x0E) | 1,
        (ticks >> 22) as u8,
        ((ticks >> 14) as u8 & 0xFE) | 1,
        (ticks >> 7) as u8,
        ((ticks << 1) as u8) | 1,
    ]
}

fn program_clock_reference(value: Timestamp) -> [u8; 6] {
    let base = value.ticks(CLOCK_RATE);
    [
        (base >> 25) as u8,
        (base >> 17) as u8,
        (base >> 9) as u8,
        (base >> 1) as u8,
        ((base as u8 & 1) << 7) | 0x7E,
        0x00,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mpegts::Demuxer,
        test_support::{FIXTURE_TS, audio_samples, video_samples},
    };

    fn demux(data: &[u8]) -> Vec<MediaEvent> {
        let mut demuxer = Demuxer::new();
        let mut events = demuxer.push(data).unwrap();
        events.extend(demuxer.finish().unwrap());
        events
    }

    #[test]
    fn round_trips_the_fixture_through_the_demuxer() {
        // Arrange
        let expected = demux(FIXTURE_TS);
        let mut muxer = Muxer::new();

        // Act
        let mut stream = BytesMut::new();
        for event in &expected {
            stream.put_slice(&muxer.push(event).unwrap());
        }
        let actual = demux(&stream);

        // Assert
        let (expected_video, actual_video) = (video_samples(&expected), video_samples(&actual));
        assert_eq!(actual_video.len(), expected_video.len());
        for (left, right) in actual_video.iter().zip(&expected_video) {
            assert_eq!(left.data, right.data);
            assert_eq!(left.is_keyframe, right.is_keyframe);
            // Conversion through the 90 kHz clock loses at most one tick.
            assert!(left.pts.micros().abs_diff(right.pts.micros()) <= 12);
            assert!(left.dts.micros().abs_diff(right.dts.micros()) <= 12);
        }
        let (expected_audio, actual_audio) = (audio_samples(&expected), audio_samples(&actual));
        assert_eq!(actual_audio.len(), expected_audio.len());
        assert!(
            actual_audio
                .iter()
                .zip(&expected_audio)
                .all(|(left, right)| left.data == right.data)
        );
    }

    #[test]
    fn writes_aligned_packets_that_start_with_the_program_tables() {
        // Arrange
        let events = demux(FIXTURE_TS);
        let mut muxer = Muxer::new();

        // Act
        let mut stream = BytesMut::new();
        for event in &events {
            stream.put_slice(&muxer.push(event).unwrap());
        }

        // Assert
        assert_eq!(stream.len() % PACKET_SIZE, 0);
        assert!(
            stream
                .chunks(PACKET_SIZE)
                .all(|packet| packet[0] == SYNC_BYTE)
        );
        let first = crate::mpegts::parser::parse_packet(&stream[..PACKET_SIZE]).unwrap();
        assert_eq!(first.pid, PAT_PID);
        assert!(first.payload_unit_start);
    }

    #[test]
    fn rejects_audio_muxed_before_its_configuration() {
        // Arrange
        let mut muxer = Muxer::new();
        let sample = AudioSample {
            data: Bytes::from_static(&[1]),
            pts: Timestamp::ZERO,
        };

        // Act / Assert
        assert!(muxer.push(&MediaEvent::Audio(sample)).is_err());
    }
    #[test]
    fn packetizes_every_payload_remainder_without_losing_bytes() {
        // Arrange
        for pcr in [None, Some(Timestamp::ZERO)] {
            for length in 1..=PACKET_SIZE * 2 {
                let payload = vec![0x42; length];
                let mut muxer = Muxer::new();

                // Act
                let packets = muxer.packetize(VIDEO_PID, &payload, pcr);
                let mut recovered = Vec::new();
                for packet in packets.chunks_exact(PACKET_SIZE) {
                    recovered.extend_from_slice(
                        crate::mpegts::parser::parse_packet(packet).unwrap().payload,
                    );
                }

                // Assert
                assert_eq!(packets.len() % PACKET_SIZE, 0);
                assert_eq!(recovered, payload, "length {length}, PCR {pcr:?}");
            }
        }
    }
}
