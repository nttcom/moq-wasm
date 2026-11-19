use anyhow::Result;
use bytes::{BufMut, Bytes, BytesMut};

use anyhow::Context;

use crate::{
    flv::tag::{Tag, TagType, encode_tag, file_header},
    h264::AvcDecoderConfigurationRecord,
    parameter_sets::NalDecoderConfig,
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
                let config = config.avc()?;
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
                let composition_offset_ms = sample.pts.millis() as i64 - sample.dts.millis() as i64;
                let mut data = BytesMut::new();
                data.put_u8(if sample.is_keyframe { 0x17 } else { 0x27 });
                data.put_u8(0x01);
                data.put_slice(&(composition_offset_ms as i32).to_be_bytes()[1..]);
                data.put_slice(
                    &AvcDecoderConfigurationRecord::sample_without_parameter_sets(
                        &sample.data,
                        nal_length_size,
                    ),
                );
                Tag {
                    tag_type: TagType::Video,
                    timestamp: sample.dts,
                    data: data.freeze(),
                }
            }
            MediaEvent::AudioConfig(config) => {
                let mut data = BytesMut::from(&AAC_SEQUENCE_HEADER[..]);
                data.put_slice(&config.aac()?.to_bytes());
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
        sample::{AudioSample, VideoConfig, VideoSample},
        test_support::{delta_frame_annexb, fixture_record, h264_aac_configs, keyframe_annexb},
    };

    fn fixture_events() -> Vec<MediaEvent> {
        let mut events = h264_aac_configs();
        events.extend([
            MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::from_millis(100),
                dts: Timestamp::from_millis(60),
            }),
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[7, 7, 7]),
                pts: Timestamp::from_millis(64),
                audio_level: None,
            }),
            MediaEvent::Video(VideoSample {
                data: delta_frame_annexb(),
                is_keyframe: false,
                pts: Timestamp::from_millis(140),
                dts: Timestamp::from_millis(100),
            }),
        ]);
        events
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
                .push(&MediaEvent::VideoConfig(VideoConfig::Avc(config)))
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
            tag_type: TagType::Other(18),
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

    mod properties {
        use proptest::{collection::vec, prelude::*};

        use super::super::Muxer;
        use crate::{
            flv::demuxer::Demuxer,
            sample::{MediaEvent, Timestamp, VideoSample},
            test_support::{delta_frame_annexb, h264_aac_configs, keyframe_annexb, video_samples},
        };

        /// A time in microseconds: a whole millisecond half the time, else
        /// anywhere within one, so the two times of a sample can round down
        /// by different amounts.
        fn micros(millis: std::ops::Range<u64>) -> impl Strategy<Value = u64> {
            (millis, prop_oneof![Just(0u64), 0..1_000u64])
                .prop_map(|(millis, micros)| millis * 1_000 + micros)
        }

        fn video() -> impl Strategy<Value = Vec<VideoSample>> {
            vec((micros(0..1 << 31), micros(0..1_000), any::<bool>()), 1..8).prop_map(|frames| {
                frames
                    .into_iter()
                    .map(|(dts, offset, is_keyframe)| VideoSample {
                        data: if is_keyframe {
                            keyframe_annexb()
                        } else {
                            delta_frame_annexb()
                        },
                        is_keyframe,
                        pts: Timestamp::from_micros(dts + offset),
                        dts: Timestamp::from_micros(dts),
                    })
                    .collect()
            })
        }

        fn in_millis(time: Timestamp) -> Timestamp {
            Timestamp::from_millis(time.millis())
        }

        proptest! {
            #[test]
            fn demuxing_muxed_video_gives_back_its_times_in_milliseconds(video in video()) {
                // Arrange
                let mut muxer = Muxer::new();
                let mut demuxer = Demuxer::new();
                let mut events = h264_aac_configs();
                events.extend(video.iter().cloned().map(MediaEvent::Video));
                let flv: Vec<u8> = events.iter().flat_map(|event| muxer.push(event).unwrap()).collect();

                // Act
                let demuxed = demuxer.push(&flv).unwrap();

                // Assert: FLV carries milliseconds, so each time rounds down
                let expected: Vec<VideoSample> = video
                    .iter()
                    .map(|sample| VideoSample {
                        pts: in_millis(sample.pts),
                        dts: in_millis(sample.dts),
                        ..sample.clone()
                    })
                    .collect();
                prop_assert_eq!(video_samples(&demuxed), expected.iter().collect::<Vec<_>>());
            }
        }
    }
}
