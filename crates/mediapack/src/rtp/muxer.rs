use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    net::IpAddr,
};

use anyhow::{Context, Result};
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig,
    h264::{AvcDecoderConfigurationRecord, annexb::nal_units},
    rtp::{
        aac::{self, AuHeaderConfig},
        h264,
        packet::Header,
        sdp::{Codec, MediaDescription, MediaKind, SessionDescription},
    },
    sample::{MediaEvent, StreamSet},
};

pub const VIDEO_PAYLOAD_TYPE: u8 = 96;
pub const AUDIO_PAYLOAD_TYPE: u8 = 97;
pub const VIDEO_CLOCK_RATE: u32 = 90_000;
/// Leaves room for the IP, UDP and RTP headers inside a 1280-byte IPv6 MTU.
const MAX_PAYLOAD: usize = 1_200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub kind: MediaKind,
    pub data: Bytes,
}

/// Packetizes media events into RTP packets without sending them. The video
/// stream is H.264 at 90 kHz, the audio stream AAC-hbr at its sample rate;
/// SSRCs, sequence numbers and timestamps start at random values (RFC 3550
/// §5.1).
pub struct Muxer {
    streams: Option<StreamSet>,
    video: StreamSender,
    audio: StreamSender,
    video_config: Option<AvcDecoderConfigurationRecord>,
    audio_config: Option<AudioSpecificConfig>,
}

struct StreamSender {
    kind: MediaKind,
    payload_type: u8,
    ssrc: u32,
    sequence_number: u16,
    timestamp_offset: u32,
}

impl Default for Muxer {
    fn default() -> Self {
        Self::new()
    }
}

impl Muxer {
    pub fn new() -> Self {
        Self {
            streams: None,
            video: StreamSender::new(MediaKind::Video, VIDEO_PAYLOAD_TYPE),
            audio: StreamSender::new(MediaKind::Audio, AUDIO_PAYLOAD_TYPE),
            video_config: None,
            audio_config: None,
        }
    }

    /// Fails on an `AudioConfig` other than AAC, and on an audio sample pushed
    /// before its `AudioConfig`, which sets the clock rate. Configuration
    /// events produce no packets.
    pub fn push(&mut self, event: &MediaEvent) -> Result<Vec<RtpPacket>> {
        match event {
            MediaEvent::Streams(streams) => {
                self.streams = Some(*streams);
                Ok(Vec::new())
            }
            MediaEvent::VideoConfig(config) => {
                self.video_config = Some(config.clone());
                Ok(Vec::new())
            }
            MediaEvent::AudioConfig(config) => {
                self.audio_config = Some(config.aac()?.clone());
                Ok(Vec::new())
            }
            MediaEvent::Video(sample) => {
                let payloads: Vec<Bytes> = nal_units(&sample.data)
                    .flat_map(|nal| h264::packetize(nal, MAX_PAYLOAD))
                    .collect();
                Ok(self
                    .video
                    .send(payloads, sample.pts.ticks(VIDEO_CLOCK_RATE)))
            }
            MediaEvent::Audio(sample) => {
                let config = self
                    .audio_config
                    .as_ref()
                    .context("audio sample before its AudioSpecificConfig")?;
                let payloads = aac::packetize(&sample.data, AuHeaderConfig::AAC_HBR, MAX_PAYLOAD)?;
                Ok(self
                    .audio
                    .send(payloads, sample.pts.ticks(config.sample_rate)))
            }
        }
    }

    /// The description a receiver needs for these packets. `None` until the
    /// `Streams` event and the configuration of every announced stream have
    /// been pushed.
    pub fn session_description(
        &self,
        destination: IpAddr,
        video_port: u16,
        audio_port: u16,
    ) -> Option<SessionDescription> {
        let streams = self.streams?;
        let mut media = Vec::new();
        if streams.has_video {
            let record = self.video_config.as_ref()?;
            media.push(MediaDescription {
                kind: MediaKind::Video,
                port: video_port,
                destination: None,
                payload_type: self.video.payload_type,
                clock_rate: VIDEO_CLOCK_RATE,
                codec: Codec::H264 {
                    parameter_sets: Some(record.clone()),
                },
            });
        }
        if streams.has_audio {
            let config = self.audio_config.as_ref()?;
            media.push(MediaDescription {
                kind: MediaKind::Audio,
                port: audio_port,
                destination: None,
                payload_type: self.audio.payload_type,
                clock_rate: config.sample_rate,
                codec: Codec::Aac {
                    config: config.clone(),
                    au_headers: AuHeaderConfig::AAC_HBR,
                },
            });
        }
        Some(SessionDescription {
            destination: Some(destination),
            media,
        })
    }
}

impl StreamSender {
    fn new(kind: MediaKind, payload_type: u8) -> Self {
        Self {
            kind,
            payload_type,
            ssrc: random() as u32,
            sequence_number: random() as u16,
            timestamp_offset: random() as u32,
        }
    }

    /// Wraps the payloads of one access unit in RTP headers; the last one
    /// carries the marker bit.
    fn send(&mut self, payloads: Vec<Bytes>, ticks: u64) -> Vec<RtpPacket> {
        let last = payloads.len().saturating_sub(1);
        payloads
            .into_iter()
            .enumerate()
            .map(|(index, payload)| {
                let header = Header {
                    marker: index == last,
                    payload_type: self.payload_type,
                    sequence_number: self.sequence_number,
                    timestamp: self.timestamp_offset.wrapping_add(ticks as u32),
                    ssrc: self.ssrc,
                };
                self.sequence_number = self.sequence_number.wrapping_add(1);
                RtpPacket {
                    kind: self.kind,
                    data: header.encode(&payload),
                }
            })
            .collect()
    }
}

/// Each `RandomState` is keyed differently, so hashing nothing with a fresh
/// one yields a fresh value without a random number crate.
fn random() -> u64 {
    RandomState::new().build_hasher().finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sample::{AudioConfig, AudioSample, Timestamp, VideoSample},
        test_support::{fixture_record, keyframe_annexb, mono_48k},
    };

    const DESTINATION: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    fn headers(packets: &[RtpPacket]) -> Vec<Header> {
        packets
            .iter()
            .map(|packet| Header::parse(&packet.data).unwrap().0)
            .collect()
    }

    #[test]
    fn sends_each_nal_unit_and_marks_the_end_of_the_access_unit() {
        // Arrange
        let mut muxer = Muxer::new();
        let keyframe = MediaEvent::Video(VideoSample {
            data: keyframe_annexb(),
            is_keyframe: true,
            pts: Timestamp::from_millis(1_000),
            dts: Timestamp::from_millis(1_000),
        });

        // Act
        let packets = muxer.push(&keyframe).unwrap();
        let headers = headers(&packets);

        // Assert: SPS, PPS and IDR share the timestamp and consecutive sequence numbers
        assert_eq!(packets.len(), 3);
        assert!(packets.iter().all(|packet| packet.kind == MediaKind::Video));
        assert_eq!(
            headers
                .iter()
                .map(|header| header.marker)
                .collect::<Vec<_>>(),
            [false, false, true]
        );
        assert!(
            headers
                .iter()
                .all(|header| header.timestamp == headers[0].timestamp)
        );
        assert_eq!(
            headers[2].sequence_number,
            headers[0].sequence_number.wrapping_add(2)
        );
        assert!(
            headers
                .iter()
                .all(|header| header.payload_type == VIDEO_PAYLOAD_TYPE)
        );
    }

    #[test]
    fn advances_timestamps_in_the_stream_clock() {
        // Arrange
        let mut muxer = Muxer::new();
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();
        let sample = |millis| {
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[0x21]),
                pts: Timestamp::from_millis(millis),
            })
        };

        // Act
        let first = headers(&muxer.push(&sample(1_000)).unwrap());
        let second = headers(&muxer.push(&sample(1_500)).unwrap());

        // Assert: 500 ms at 48 kHz
        assert_eq!(second[0].timestamp.wrapping_sub(first[0].timestamp), 24_000);
        assert!(first[0].marker);
        assert_eq!(first[0].payload_type, AUDIO_PAYLOAD_TYPE);
    }

    #[test]
    fn rejects_audio_other_than_aac() {
        // Arrange
        let mut muxer = Muxer::new();
        let opus = MediaEvent::AudioConfig(AudioConfig::Opus {
            sample_rate: 48_000,
            channels: 2,
        });

        // Act / Assert
        assert!(muxer.push(&opus).is_err());
    }

    #[test]
    fn needs_the_audio_config_before_audio_samples() {
        // Arrange
        let mut muxer = Muxer::new();

        // Act / Assert
        assert!(
            muxer
                .push(&MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[0x21]),
                    pts: Timestamp::ZERO,
                }))
                .is_err()
        );
    }

    #[test]
    fn describes_the_session_once_every_stream_is_configured() {
        // Arrange
        let mut muxer = Muxer::new();
        muxer
            .push(&MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true,
            }))
            .unwrap();
        muxer
            .push(&MediaEvent::VideoConfig(fixture_record()))
            .unwrap();

        // Act
        let without_audio = muxer.session_description(DESTINATION, 5_004, 5_006);
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();
        let description = muxer
            .session_description(DESTINATION, 5_004, 5_006)
            .unwrap();

        // Assert
        assert!(without_audio.is_none());
        assert_eq!(description.destination, Some(DESTINATION));
        assert_eq!(description.media.len(), 2);
        assert_eq!(description.media[0].port, 5_004);
        assert_eq!(description.media[1].port, 5_006);
        assert_eq!(description.media[1].clock_rate, 48_000);
    }

    #[test]
    fn describes_a_video_only_session_without_audio_config() {
        // Arrange
        let mut muxer = Muxer::new();
        muxer
            .push(&MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: false,
            }))
            .unwrap();
        muxer
            .push(&MediaEvent::VideoConfig(fixture_record()))
            .unwrap();

        // Act
        let description = muxer
            .session_description(DESTINATION, 5_004, 5_006)
            .unwrap();

        // Assert
        assert_eq!(description.media.len(), 1);
        assert_eq!(description.media[0].kind, MediaKind::Video);
    }
}
