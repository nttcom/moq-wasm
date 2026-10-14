use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    net::IpAddr,
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;

use crate::{
    h264::{AvcDecoderConfigurationRecord, annexb::nal_units},
    rtp::{
        aac::{self, AuHeaderConfig},
        h264,
        packet::Header,
        sdp::{
            Codec, MediaDescription, MediaKind, OPUS_CLOCK_RATE, SessionDescription, SsrcAttribute,
        },
    },
    sample::{AudioConfig, MediaEvent, StreamSet},
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

/// The payload type and SSRC of each stream. An SSRC left `None` is drawn at
/// random, as are the first sequence number and timestamp (RFC 3550 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxerOptions {
    pub video_payload_type: u8,
    pub audio_payload_type: u8,
    pub video_ssrc: Option<u32>,
    pub audio_ssrc: Option<u32>,
}

impl Default for MuxerOptions {
    fn default() -> Self {
        Self {
            video_payload_type: VIDEO_PAYLOAD_TYPE,
            audio_payload_type: AUDIO_PAYLOAD_TYPE,
            video_ssrc: None,
            audio_ssrc: None,
        }
    }
}

/// Packetizes media events into RTP packets without sending them. The video
/// stream is H.264 at 90 kHz; the audio stream is AAC-hbr or A-law at its
/// sample rate, or Opus at 48 kHz.
pub struct Muxer {
    streams: Option<StreamSet>,
    /// One canonical name for both streams, so a receiver synchronizes them.
    cname: String,
    video: StreamSender,
    audio: StreamSender,
    video_config: Option<AvcDecoderConfigurationRecord>,
    audio_config: Option<AudioConfig>,
}

struct StreamSender {
    kind: MediaKind,
    payload_type: u8,
    ssrc: u32,
    sequence_number: u16,
    timestamp_offset: u32,
    started: bool,
}

impl Default for Muxer {
    fn default() -> Self {
        Self::new(MuxerOptions::default())
    }
}

impl Muxer {
    /// Panics when a payload type does not fit the 7 bits of the RTP header.
    pub fn new(options: MuxerOptions) -> Self {
        assert!(
            options.video_payload_type <= 127 && options.audio_payload_type <= 127,
            "RTP payload types are 7 bits"
        );
        Self {
            streams: None,
            cname: format!("{:016x}", random()),
            video: StreamSender::new(
                MediaKind::Video,
                options.video_payload_type,
                options.video_ssrc,
            ),
            audio: StreamSender::new(
                MediaKind::Audio,
                options.audio_payload_type,
                options.audio_ssrc,
            ),
            video_config: None,
            audio_config: None,
        }
    }

    /// Fails on an MP3 `AudioConfig`, which has no RTP payload format here, on
    /// A-law with no channels, and on an audio sample pushed before its
    /// `AudioConfig`. Configuration events produce no packets.
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
                match config {
                    AudioConfig::Mp3 { .. } => bail!("MP3 audio has no RTP payload format here"),
                    AudioConfig::Pcma { channels: 0, .. } => bail!("A-law audio with no channels"),
                    _ => {}
                }
                self.audio_config = Some(config.clone());
                Ok(Vec::new())
            }
            MediaEvent::Video(sample) => {
                let payloads: Vec<Bytes> = nal_units(&sample.data)
                    .flat_map(|nal| h264::packetize(nal, MAX_PAYLOAD))
                    .collect();
                Ok(self
                    .video
                    .send_access_unit(payloads, sample.pts.ticks(VIDEO_CLOCK_RATE)))
            }
            MediaEvent::Audio(sample) => {
                let config = self
                    .audio_config
                    .as_ref()
                    .context("audio sample before its AudioConfig")?;
                let ticks = sample.pts.ticks(audio_clock_rate(config));
                match config {
                    AudioConfig::Aac(_) => {
                        let payloads =
                            aac::packetize(&sample.data, AuHeaderConfig::AAC_HBR, MAX_PAYLOAD)?;
                        Ok(self.audio.send_access_unit(payloads, ticks))
                    }
                    // One A-law byte per sample and channel, so a frame larger
                    // than a packet splits at any sample boundary.
                    AudioConfig::Pcma { channels, .. } => {
                        let samples_per_packet = MAX_PAYLOAD / *channels as usize;
                        Ok(sample
                            .data
                            .chunks(samples_per_packet * *channels as usize)
                            .enumerate()
                            .map(|(index, chunk)| {
                                let offset = (index * samples_per_packet) as u64;
                                self.audio
                                    .send_frame(sample.data.slice_ref(chunk), ticks + offset)
                            })
                            .collect())
                    }
                    // RFC 7587 §4.2 puts one Opus packet in each RTP packet.
                    AudioConfig::Opus { .. } => {
                        Ok(vec![self.audio.send_frame(sample.data.clone(), ticks)])
                    }
                    AudioConfig::Mp3 { .. } => unreachable!("rejected at its AudioConfig"),
                }
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
                ssrc: Some(self.ssrc_attribute(&self.video)),
            });
        }
        if streams.has_audio {
            let config = self.audio_config.as_ref()?;
            media.push(MediaDescription {
                kind: MediaKind::Audio,
                port: audio_port,
                destination: None,
                payload_type: self.audio.payload_type,
                clock_rate: audio_clock_rate(config),
                codec: match config {
                    AudioConfig::Aac(config) => Codec::Aac {
                        config: config.clone(),
                        au_headers: AuHeaderConfig::AAC_HBR,
                    },
                    AudioConfig::Pcma { channels, .. } => Codec::Pcma {
                        channels: *channels,
                    },
                    AudioConfig::Opus { channels, .. } => Codec::Opus {
                        channels: *channels,
                    },
                    AudioConfig::Mp3 { .. } => unreachable!("rejected at its AudioConfig"),
                },
                ssrc: Some(self.ssrc_attribute(&self.audio)),
            });
        }
        Some(SessionDescription {
            destination: Some(destination),
            media,
        })
    }

    fn ssrc_attribute(&self, stream: &StreamSender) -> SsrcAttribute {
        SsrcAttribute {
            ssrc: stream.ssrc,
            cname: self.cname.clone(),
        }
    }
}

impl StreamSender {
    fn new(kind: MediaKind, payload_type: u8, ssrc: Option<u32>) -> Self {
        Self {
            kind,
            payload_type,
            ssrc: ssrc.unwrap_or_else(|| random() as u32),
            sequence_number: random() as u16,
            timestamp_offset: random() as u32,
            started: false,
        }
    }

    /// Wraps the payloads of one H.264 or AAC access unit in RTP headers; the
    /// last one carries the marker bit (RFC 6184 §5.1, RFC 3640 §3.1).
    fn send_access_unit(&mut self, payloads: Vec<Bytes>, ticks: u64) -> Vec<RtpPacket> {
        let last = payloads.len().saturating_sub(1);
        payloads
            .into_iter()
            .enumerate()
            .map(|(index, payload)| self.packet(&payload, ticks, index == last))
            .collect()
    }

    /// Sends one A-law or Opus packet. The marker bit starts a talkspurt
    /// (RFC 3551 §4.1), which only the stream's first packet does here.
    fn send_frame(&mut self, payload: Bytes, ticks: u64) -> RtpPacket {
        let marker = !self.started;
        self.packet(&payload, ticks, marker)
    }

    fn packet(&mut self, payload: &[u8], ticks: u64, marker: bool) -> RtpPacket {
        let header = Header {
            marker,
            payload_type: self.payload_type,
            sequence_number: self.sequence_number,
            timestamp: self.timestamp_offset.wrapping_add(ticks as u32),
            ssrc: self.ssrc,
        };
        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.started = true;
        RtpPacket {
            kind: self.kind,
            data: header.encode(payload),
        }
    }
}

fn audio_clock_rate(config: &AudioConfig) -> u32 {
    match config {
        AudioConfig::Opus { .. } => OPUS_CLOCK_RATE,
        other => other.sample_rate(),
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
        let mut muxer = Muxer::default();
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
        let mut muxer = Muxer::default();
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
    fn uses_the_given_payload_types_and_ssrcs() {
        // Arrange
        let mut muxer = Muxer::new(MuxerOptions {
            video_payload_type: 102,
            audio_payload_type: 111,
            video_ssrc: Some(2_222),
            audio_ssrc: None,
        });
        muxer
            .push(&MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true,
            }))
            .unwrap();
        muxer
            .push(&MediaEvent::VideoConfig(fixture_record()))
            .unwrap();
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();

        // Act
        let video = headers(
            &muxer
                .push(&MediaEvent::Video(VideoSample {
                    data: keyframe_annexb(),
                    is_keyframe: true,
                    pts: Timestamp::ZERO,
                    dts: Timestamp::ZERO,
                }))
                .unwrap(),
        );
        let description = muxer
            .session_description(DESTINATION, 5_004, 5_006)
            .unwrap();

        // Assert: both streams share one cname so a receiver can align them
        assert_eq!((video[0].payload_type, video[0].ssrc), (102, 2_222));
        let [video_media, audio_media] = &description.media[..] else {
            panic!("expected a video and an audio section");
        };
        assert_eq!(audio_media.payload_type, 111);
        let (video_ssrc, audio_ssrc) = (
            video_media.ssrc.as_ref().unwrap(),
            audio_media.ssrc.as_ref().unwrap(),
        );
        assert_eq!(video_ssrc.ssrc, 2_222);
        assert_eq!(video_ssrc.cname, audio_ssrc.cname);
    }

    #[test]
    fn rejects_audio_it_cannot_packetize() {
        // Arrange
        let mut muxer = Muxer::default();
        let mp3 = MediaEvent::AudioConfig(AudioConfig::Mp3 {
            sample_rate: 44_100,
            channels: 2,
        });
        let silent = MediaEvent::AudioConfig(AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 0,
        });

        // Act / Assert
        assert!(muxer.push(&mp3).is_err());
        assert!(muxer.push(&silent).is_err());
    }

    #[test]
    fn splits_a_law_frames_at_sample_boundaries() {
        // Arrange: 2 400 bytes of stereo A-law, 1 200 samples per channel
        let mut muxer = Muxer::default();
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Pcma {
                sample_rate: 8_000,
                channels: 2,
            }))
            .unwrap();
        let frame = MediaEvent::Audio(AudioSample {
            data: Bytes::from(vec![0xD5; 2_400]),
            pts: Timestamp::from_millis(1_000),
        });
        let next = MediaEvent::Audio(AudioSample {
            data: Bytes::from(vec![0xD5; 160]),
            pts: Timestamp::from_millis(1_150),
        });

        // Act
        let packets = muxer.push(&frame).unwrap();
        let following = headers(&muxer.push(&next).unwrap());
        let first = headers(&packets);

        // Assert: 600 samples per packet; only the stream's first packet starts a talkspurt
        assert_eq!(packets.len(), 2);
        assert!(packets.iter().all(|packet| packet.data.len() == 12 + 1_200));
        assert_eq!(first[1].timestamp.wrapping_sub(first[0].timestamp), 600);
        assert_eq!(
            following[0].timestamp.wrapping_sub(first[0].timestamp),
            1_200
        );
        assert_eq!(
            [first[0].marker, first[1].marker, following[0].marker],
            [true, false, false]
        );
    }

    #[test]
    fn sends_each_opus_packet_whole_on_the_48_khz_clock() {
        // Arrange: a 16 kHz Opus stream still counts RTP time at 48 kHz
        let mut muxer = Muxer::default();
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Opus {
                sample_rate: 16_000,
                channels: 1,
            }))
            .unwrap();
        let packet = |millis| {
            MediaEvent::Audio(AudioSample {
                data: Bytes::from(vec![0x78; 1_400]),
                pts: Timestamp::from_millis(millis),
            })
        };

        // Act
        let first = muxer.push(&packet(1_000)).unwrap();
        let second = muxer.push(&packet(1_020)).unwrap();

        // Assert
        assert_eq!((first.len(), first[0].data.len()), (1, 12 + 1_400));
        assert_eq!(
            headers(&second)[0]
                .timestamp
                .wrapping_sub(headers(&first)[0].timestamp),
            960
        );
    }

    #[test]
    fn describes_a_law_and_opus_streams() {
        // Arrange
        let describe = |config: AudioConfig| {
            let mut muxer = Muxer::default();
            muxer
                .push(&MediaEvent::Streams(StreamSet {
                    has_video: false,
                    has_audio: true,
                }))
                .unwrap();
            muxer.push(&MediaEvent::AudioConfig(config)).unwrap();
            let media = muxer
                .session_description(DESTINATION, 5_004, 5_006)
                .unwrap()
                .media
                .remove(0);
            (media.codec, media.clock_rate)
        };

        // Act
        let pcma = describe(AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        });
        let opus = describe(AudioConfig::Opus {
            sample_rate: 24_000,
            channels: 2,
        });

        // Assert
        assert_eq!(pcma, (Codec::Pcma { channels: 1 }, 8_000));
        assert_eq!(opus, (Codec::Opus { channels: 2 }, 48_000));
    }

    #[test]
    fn needs_the_audio_config_before_audio_samples() {
        // Arrange
        let mut muxer = Muxer::default();

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
        let mut muxer = Muxer::default();
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
        let mut muxer = Muxer::default();
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
