use anyhow::Result;
use bytes::{BufMut, Bytes, BytesMut};

use anyhow::Context;

use crate::{
    flv::tag::{Tag, TagType, encode_tag, file_header},
    h264::{AvcDecoderConfigurationRecord, annexb::annexb_to_avcc_without_parameter_sets},
    sample::{MediaEvent, StreamSet, Timestamp},
};

const AVC_SEQUENCE_HEADER: [u8; 5] = [0x17, 0x00, 0, 0, 0];
const AAC_SEQUENCE_HEADER: [u8; 2] = [0xAF, 0x00];
const AAC_RAW: [u8; 2] = [0xAF, 0x01];

pub struct Muxer {
    header_written: bool,
    streams: StreamSet,
    video_config: Option<AvcDecoderConfigurationRecord>,
    last_video_timestamp: Timestamp,
}

impl Default for Muxer {
    fn default() -> Self {
        Self {
            header_written: false,
            streams: StreamSet {
                has_video: true,
                has_audio: true,
            },
            video_config: None,
            last_video_timestamp: Timestamp::ZERO,
        }
    }
}

impl Muxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: &MediaEvent) -> Result<Bytes> {
        let tag = match event {
            MediaEvent::Streams(streams) => {
                if !self.header_written {
                    self.streams = *streams;
                }
                return Ok(Bytes::new());
            }
            MediaEvent::VideoConfig(config) => {
                self.video_config = Some(config.clone());
                let mut data = BytesMut::from(&AVC_SEQUENCE_HEADER[..]);
                data.put_slice(&config.to_bytes());
                Tag {
                    tag_type: TagType::Video,
                    timestamp: self.last_video_timestamp,
                    data: data.freeze(),
                }
            }
            MediaEvent::Video(sample) => {
                let nal_length_size = self
                    .video_config
                    .as_ref()
                    .context("video sample before its configuration")?
                    .nal_length_size as usize;
                self.last_video_timestamp = sample.dts;
                let composition_offset_ms =
                    (sample.pts.micros() as i64 - sample.dts.micros() as i64) / 1_000;
                let mut data = BytesMut::new();
                data.put_u8(if sample.is_keyframe { 0x17 } else { 0x27 });
                data.put_u8(0x01);
                data.put_slice(&(composition_offset_ms as i32).to_be_bytes()[1..]);
                data.put_slice(&annexb_to_avcc_without_parameter_sets(
                    &sample.data,
                    nal_length_size,
                ));
                Tag {
                    tag_type: TagType::Video,
                    timestamp: sample.dts,
                    data: data.freeze(),
                }
            }
            MediaEvent::AudioConfig(config) => {
                let mut data = BytesMut::from(&AAC_SEQUENCE_HEADER[..]);
                data.put_slice(&config.to_bytes());
                Tag {
                    tag_type: TagType::Audio,
                    timestamp: Timestamp::ZERO,
                    data: data.freeze(),
                }
            }
            MediaEvent::Audio(sample) => {
                let mut data = BytesMut::from(&AAC_RAW[..]);
                data.put_slice(&sample.data);
                Tag {
                    tag_type: TagType::Audio,
                    timestamp: sample.pts,
                    data: data.freeze(),
                }
            }
        };
        Ok(self.push_tag(&tag))
    }

    pub fn push_tag(&mut self, tag: &Tag) -> Bytes {
        let mut out = BytesMut::new();
        if !self.header_written {
            out.put_slice(&file_header(self.streams));
            self.header_written = true;
        }
        out.put_slice(&encode_tag(tag));
        out.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        flv::demuxer::Demuxer,
        sample::{AudioSample, VideoSample},
        test_support::{delta_frame_annexb, fixture_record, keyframe_annexb, mono_48k},
    };

    fn fixture_events() -> Vec<MediaEvent> {
        vec![
            MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true,
            }),
            MediaEvent::VideoConfig(fixture_record()),
            MediaEvent::AudioConfig(mono_48k()),
            MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::from_millis(100),
                dts: Timestamp::from_millis(60),
            }),
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[7, 7, 7]),
                pts: Timestamp::from_millis(64),
            }),
            MediaEvent::Video(VideoSample {
                data: delta_frame_annexb(),
                is_keyframe: false,
                pts: Timestamp::from_millis(140),
                dts: Timestamp::from_millis(100),
            }),
        ]
    }

    #[test]
    fn round_trips_events_through_demuxer() {
        // Arrange
        let events = fixture_events();
        let mut muxer = Muxer::new();
        let mut demuxer = Demuxer::new();

        // Act
        let mut flv = BytesMut::new();
        for event in &events {
            flv.put_slice(&muxer.push(event).unwrap());
        }
        let decoded = demuxer.push(&flv).unwrap();

        // Assert
        assert_eq!(decoded, events);
    }

    #[test]
    fn length_prefixes_with_the_configured_nal_length_size() {
        // Arrange
        let mut config = fixture_record();
        config.nal_length_size = 2;
        let mut muxer = Muxer::new();
        let mut demuxer = Demuxer::new();
        let sample = MediaEvent::Video(VideoSample {
            data: delta_frame_annexb(),
            is_keyframe: false,
            pts: Timestamp::from_millis(40),
            dts: Timestamp::from_millis(40),
        });

        // Act
        let mut flv = BytesMut::from(
            muxer
                .push(&MediaEvent::VideoConfig(config))
                .unwrap()
                .as_ref(),
        );
        flv.put_slice(&muxer.push(&sample).unwrap());
        let decoded = demuxer.push(&flv).unwrap();

        // Assert
        assert_eq!(decoded.last(), Some(&sample));
    }

    #[test]
    fn rejects_video_before_configuration() {
        // Arrange
        let mut muxer = Muxer::new();
        let sample = fixture_events().pop().unwrap();

        // Act / Assert
        assert!(muxer.push(&sample).is_err());
    }

    #[test]
    fn writes_file_header_only_once() {
        // Arrange
        let mut muxer = Muxer::new();
        let tag = Tag {
            tag_type: TagType::ScriptData,
            timestamp: Timestamp::ZERO,
            data: Bytes::from_static(&[1]),
        };

        // Act
        let first = muxer.push_tag(&tag);
        let second = muxer.push_tag(&tag);

        // Assert
        assert!(first.starts_with(b"FLV"));
        assert_eq!(first.len(), 13 + 11 + 1 + 4);
        assert_eq!(second.len(), 11 + 1 + 4);
    }
}
