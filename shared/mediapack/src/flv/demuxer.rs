use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};

use crate::{
    aac::AudioSpecificConfig,
    flv::tag::{Tag, TagType, parse_file_header, parse_tag},
    h264::{
        AvcDecoderConfigurationRecord, NalUnitType, annexb::nal_units, avcc::avcc_to_annexb,
        nal::nal_unit_type,
    },
    sample::{AudioSample, MediaEvent, Timestamp, VideoSample},
};

const VIDEO_CODEC_AVC: u8 = 7;
const AUDIO_FORMAT_AAC: u8 = 10;
const FRAME_TYPE_KEY: u8 = 1;
const FRAME_TYPE_COMMAND: u8 = 5;
const PACKET_TYPE_SEQUENCE_HEADER: u8 = 0;
const PACKET_TYPE_PAYLOAD: u8 = 1;

#[derive(Default)]
pub struct Demuxer {
    buffer: BytesMut,
    header_parsed: bool,
    video_config: Option<AvcDecoderConfigurationRecord>,
    audio_config: Option<AudioSpecificConfig>,
}

impl Demuxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<MediaEvent>> {
        self.buffer.extend_from_slice(data);
        let mut events = Vec::new();
        if !self.header_parsed {
            let Some((streams, body_start)) = parse_file_header(&self.buffer)? else {
                return Ok(events);
            };
            let _ = self.buffer.split_to(body_start);
            self.header_parsed = true;
            events.push(MediaEvent::Streams(streams));
        }
        while let Some((tag, consumed)) = parse_tag(&self.buffer)? {
            let _ = self.buffer.split_to(consumed);
            events.extend(self.push_tag(&tag)?);
        }
        Ok(events)
    }

    pub fn push_tag(&mut self, tag: &Tag) -> Result<Vec<MediaEvent>> {
        match tag.tag_type {
            TagType::Video => self.push_video_tag(tag.timestamp, &tag.data),
            TagType::Audio => self.push_audio_tag(tag.timestamp, &tag.data),
            TagType::Other(_) => Ok(Vec::new()),
        }
    }

    fn push_video_tag(&mut self, timestamp: Timestamp, data: &[u8]) -> Result<Vec<MediaEvent>> {
        let Some((&flags, rest)) = data.split_first() else {
            return Ok(Vec::new());
        };
        let frame_type = flags >> 4;
        if flags & 0x0F != VIDEO_CODEC_AVC || frame_type == FRAME_TYPE_COMMAND || rest.len() < 4 {
            return Ok(Vec::new());
        }
        let packet_type = rest[0];
        let composition_offset_ms = i32::from_be_bytes([0, rest[1], rest[2], rest[3]]) << 8 >> 8;
        let body = &rest[4..];
        match packet_type {
            PACKET_TYPE_SEQUENCE_HEADER => {
                let config = AvcDecoderConfigurationRecord::parse(body)
                    .context("parse FLV AVC sequence header")?;
                self.video_config = Some(config.clone());
                Ok(vec![MediaEvent::VideoConfig(config)])
            }
            PACKET_TYPE_PAYLOAD => {
                let config = self
                    .video_config
                    .as_ref()
                    .context("FLV AVC NALU received before sequence header")?;
                let annexb = avcc_to_annexb(body, config.nal_length_size as usize)?;
                let is_keyframe = frame_type == FRAME_TYPE_KEY;
                let data = if is_keyframe && !contains_sps(&annexb) {
                    Bytes::from([config.parameter_sets_annexb().as_ref(), &annexb].concat())
                } else {
                    annexb
                };
                let pts_micros = timestamp.micros() as i64 + composition_offset_ms as i64 * 1_000;
                Ok(vec![MediaEvent::Video(VideoSample {
                    data,
                    is_keyframe,
                    pts: Timestamp::from_micros(pts_micros.max(0) as u64),
                    dts: timestamp,
                })])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn push_audio_tag(&mut self, timestamp: Timestamp, data: &[u8]) -> Result<Vec<MediaEvent>> {
        let Some((&flags, rest)) = data.split_first() else {
            return Ok(Vec::new());
        };
        if flags >> 4 != AUDIO_FORMAT_AAC || rest.is_empty() {
            return Ok(Vec::new());
        }
        let body = &rest[1..];
        match rest[0] {
            PACKET_TYPE_SEQUENCE_HEADER => {
                let config =
                    AudioSpecificConfig::parse(body).context("parse FLV AAC sequence header")?;
                self.audio_config = Some(config.clone());
                Ok(vec![MediaEvent::AudioConfig(config)])
            }
            PACKET_TYPE_PAYLOAD => Ok(vec![MediaEvent::Audio(AudioSample {
                data: Bytes::copy_from_slice(body),
                pts: timestamp,
            })]),
            _ => Ok(Vec::new()),
        }
    }
}

fn contains_sps(annexb: &[u8]) -> bool {
    nal_units(annexb).any(|nal| nal_unit_type(nal) == Some(NalUnitType::Sps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        h264::annexb::annexb_to_avcc,
        sample::StreamSet,
        test_support::{
            FIXTURE_FLV, IDR_SLICE, audio_samples, count_events, delta_frame_annexb,
            fixture_record, keyframe_annexb, mono_48k, video_samples,
        },
    };

    fn video_tag(timestamp_ms: u64, payload: &[u8]) -> Tag {
        Tag {
            tag_type: TagType::Video,
            timestamp: Timestamp::from_millis(timestamp_ms),
            data: Bytes::copy_from_slice(payload),
        }
    }

    fn sequence_header_tag() -> Tag {
        let mut payload = vec![0x17, 0x00, 0, 0, 0];
        payload.extend_from_slice(&fixture_record().to_bytes());
        video_tag(0, &payload)
    }

    fn nalu_tag(timestamp_ms: u64, is_keyframe: bool, cts_ms: i32, annexb: &[u8]) -> Tag {
        let mut payload = vec![if is_keyframe { 0x17 } else { 0x27 }, 0x01];
        payload.extend_from_slice(&cts_ms.to_be_bytes()[1..]);
        payload.extend_from_slice(&annexb_to_avcc(annexb, 4));
        video_tag(timestamp_ms, &payload)
    }

    #[test]
    fn converts_avc_tags_to_annexb_samples() {
        // Arrange
        let mut demuxer = Demuxer::new();
        let bare_idr = crate::h264::annexb::with_start_codes([&IDR_SLICE[..]]);

        // Act
        let config_events = demuxer.push_tag(&sequence_header_tag()).unwrap();
        let key_events = demuxer.push_tag(&nalu_tag(40, true, 0, &bare_idr)).unwrap();
        let delta_events = demuxer
            .push_tag(&nalu_tag(80, false, -40, &delta_frame_annexb()))
            .unwrap();

        // Assert
        assert_eq!(config_events, [MediaEvent::VideoConfig(fixture_record())]);
        assert_eq!(
            key_events,
            [MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::from_millis(40),
                dts: Timestamp::from_millis(40),
            })]
        );
        assert_eq!(
            delta_events,
            [MediaEvent::Video(VideoSample {
                data: delta_frame_annexb(),
                is_keyframe: false,
                pts: Timestamp::from_millis(40),
                dts: Timestamp::from_millis(80),
            })]
        );
    }

    #[test]
    fn rejects_nalu_before_sequence_header() {
        // Arrange
        let mut demuxer = Demuxer::new();

        // Act
        let result = demuxer.push_tag(&nalu_tag(0, true, 0, &keyframe_annexb()));

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn converts_aac_tags_to_audio_samples() {
        // Arrange
        let mut demuxer = Demuxer::new();
        let mut header = vec![0xAF, 0x00];
        header.extend_from_slice(&mono_48k().to_bytes());
        let audio_tag = |timestamp_ms, data: &[u8]| Tag {
            tag_type: TagType::Audio,
            timestamp: Timestamp::from_millis(timestamp_ms),
            data: Bytes::copy_from_slice(data),
        };

        // Act
        let config_events = demuxer.push_tag(&audio_tag(0, &header)).unwrap();
        let sample_events = demuxer
            .push_tag(&audio_tag(21, &[0xAF, 0x01, 9, 8]))
            .unwrap();

        // Assert
        assert_eq!(config_events, [MediaEvent::AudioConfig(mono_48k())]);
        assert_eq!(
            sample_events,
            [MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[9, 8]),
                pts: Timestamp::from_millis(21),
            })]
        );
    }

    #[test]
    fn demuxes_ffmpeg_generated_file_in_chunks() {
        // Arrange
        let mut demuxer = Demuxer::new();
        let mut events = Vec::new();

        // Act
        for chunk in FIXTURE_FLV.chunks(701) {
            events.extend(demuxer.push(chunk).unwrap());
        }

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true
            })
        );
        let video = video_samples(&events);
        assert_eq!(video.len(), 9);
        assert_eq!(video.iter().filter(|sample| sample.is_keyframe).count(), 2);
        assert_eq!(audio_samples(&events).len(), 30);
        assert!(video[0].is_keyframe);
        assert!(contains_sps(&video[0].data));
        assert_eq!(
            count_events(&events, |event| matches!(event, MediaEvent::VideoConfig(_))),
            1
        );
        assert_eq!(
            count_events(&events, |event| matches!(event, MediaEvent::AudioConfig(_))),
            1
        );
    }
}
