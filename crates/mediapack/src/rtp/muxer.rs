use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    net::{IpAddr, SocketAddr},
};

use anyhow::{Context, Result, bail};
use bytes::Bytes;

use crate::{
    h264::{AvcDecoderConfigurationRecord, annexb::nal_units},
    rtp::{
        aac::{self, AuHeaderConfig},
        h264,
        packet::Header,
        rtcp::SenderReport,
        sdp::{
            Codec, MediaDescription, MediaKind, OPUS_CLOCK_RATE, SessionDescription, SsrcAttribute,
        },
    },
    sample::{AudioConfig, MediaEvent, StreamSet, Timestamp},
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

/// An RTCP compound packet for the stream `kind`; it goes to that stream's
/// RTCP port, see [`Destination::rtcp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpPacket {
    pub kind: MediaKind,
    pub data: Bytes,
}

/// Where the streams go. Each stream takes its own port, and RTCP either the
/// same port (`rtcp_mux`, RFC 5761) or the one above it (RFC 3550 §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination {
    pub address: IpAddr,
    pub video_port: u16,
    pub audio_port: u16,
    pub rtcp_mux: bool,
}

impl Destination {
    pub fn rtp(&self, kind: MediaKind) -> SocketAddr {
        SocketAddr::new(self.address, self.port(kind))
    }

    pub fn rtcp(&self, kind: MediaKind) -> SocketAddr {
        let port = self.port(kind);
        SocketAddr::new(
            self.address,
            if self.rtcp_mux {
                port
            } else {
                port.wrapping_add(1)
            },
        )
    }

    fn port(&self, kind: MediaKind) -> u16 {
        match kind {
            MediaKind::Video => self.video_port,
            MediaKind::Audio => self.audio_port,
        }
    }
}

/// An SSRC left `None` is drawn at random, as are each stream's first
/// sequence number and timestamp (RFC 3550 §5.1).
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
    packet_count: u32,
    octet_count: u32,
    latest_sample: Option<LatestSample>,
}

/// What a sender report extrapolates the stream's RTP time from.
#[derive(Clone, Copy)]
struct LatestSample {
    rtp_timestamp: u32,
    pts: Timestamp,
    clock_rate: u32,
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

    /// Fails on an MP3 `AudioConfig`, which this muxer has no RTP payload
    /// format for, on A-law with no channels, and on an audio sample pushed
    /// before its `AudioConfig`. Configuration events produce no packets.
    pub fn push(&mut self, event: &MediaEvent) -> Result<Vec<RtpPacket>> {
        match event {
            MediaEvent::Streams(streams) => {
                self.streams = Some(*streams);
                Ok(Vec::new())
            }
            MediaEvent::VideoConfig(config) => {
                let config = config.avc()?;
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
                let ticks = sample.pts.ticks(VIDEO_CLOCK_RATE);
                self.video.note_sample(ticks, sample.pts, VIDEO_CLOCK_RATE);
                Ok(self.video.send_access_unit(payloads, ticks))
            }
            MediaEvent::Audio(sample) => {
                let config = self
                    .audio_config
                    .as_ref()
                    .context("audio sample before its AudioConfig")?;
                let clock_rate = audio_clock_rate(config);
                let ticks = sample.pts.ticks(clock_rate);
                self.audio.note_sample(ticks, sample.pts, clock_rate);
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
    pub fn session_description(&self, destination: &Destination) -> Option<SessionDescription> {
        let streams = self.streams?;
        let mut media = Vec::new();
        if streams.has_video {
            let record = self.video_config.as_ref()?;
            media.push(MediaDescription {
                kind: MediaKind::Video,
                port: destination.video_port,
                destination: None,
                payload_type: self.video.payload_type,
                clock_rate: VIDEO_CLOCK_RATE,
                codec: Codec::H264 {
                    parameter_sets: Some(record.clone()),
                },
                ssrc: Some(self.ssrc_attribute(&self.video)),
                rtcp_mux: destination.rtcp_mux,
            });
        }
        if streams.has_audio {
            let config = self.audio_config.as_ref()?;
            media.push(MediaDescription {
                kind: MediaKind::Audio,
                port: destination.audio_port,
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
                rtcp_mux: destination.rtcp_mux,
            });
        }
        Some(SessionDescription {
            destination: Some(destination.address),
            media,
        })
    }

    /// One RTCP sender report per stream that has sent a sample, to be sent
    /// at `now`. RFC 3550 §6.4.1 pairs the send time with the RTP timestamp
    /// of that same instant, so each report extrapolates the stream's latest
    /// RTP timestamp from the sample's wall-clock time `capture_origin + pts`
    /// to `now`. Both are microseconds since the Unix epoch, and
    /// `capture_origin` must be the time presentation time zero stands for on
    /// every stream, which is what keeps the streams aligned.
    pub fn sender_reports(&self, capture_origin: Timestamp, now: Timestamp) -> Vec<RtcpPacket> {
        [&self.video, &self.audio]
            .into_iter()
            .filter_map(|stream| {
                let sample = stream.latest_sample?;
                let captured = capture_origin.saturating_add(sample.pts);
                let elapsed_micros = now.micros() as i128 - captured.micros() as i128;
                let elapsed_ticks = elapsed_micros * sample.clock_rate as i128 / 1_000_000;
                let report = SenderReport {
                    ssrc: stream.ssrc,
                    wallclock: now,
                    // The RTP clock wraps, so a negative elapsed time (a sender
                    // clock behind the publisher's) wraps back as well.
                    rtp_timestamp: sample.rtp_timestamp.wrapping_add(elapsed_ticks as u32),
                    packet_count: stream.packet_count,
                    octet_count: stream.octet_count,
                    cname: &self.cname,
                };
                Some(RtcpPacket {
                    kind: stream.kind,
                    data: report.encode(),
                })
            })
            .collect()
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
            packet_count: 0,
            octet_count: 0,
            latest_sample: None,
        }
    }

    fn note_sample(&mut self, ticks: u64, pts: Timestamp, clock_rate: u32) {
        self.latest_sample = Some(LatestSample {
            rtp_timestamp: self.rtp_timestamp(ticks),
            pts,
            clock_rate,
        });
    }

    fn rtp_timestamp(&self, ticks: u64) -> u32 {
        self.timestamp_offset.wrapping_add(ticks as u32)
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

    /// Wraps one A-law or Opus payload in an RTP header. The marker bit flags
    /// the first packet of a talkspurt (RFC 3551 §4.1); without silence
    /// suppression only the stream's first packet is one.
    fn send_frame(&mut self, payload: Bytes, ticks: u64) -> RtpPacket {
        let marker = self.packet_count == 0;
        self.packet(&payload, ticks, marker)
    }

    fn packet(&mut self, payload: &[u8], ticks: u64, marker: bool) -> RtpPacket {
        let header = Header {
            marker,
            payload_type: self.payload_type,
            sequence_number: self.sequence_number,
            timestamp: self.rtp_timestamp(ticks),
            ssrc: self.ssrc,
        };
        self.sequence_number = self.sequence_number.wrapping_add(1);
        // RFC 3550 §6.4.1 counts payload octets only, without header or
        // padding; both 32-bit counts wrap.
        self.packet_count = self.packet_count.wrapping_add(1);
        self.octet_count = self.octet_count.wrapping_add(payload.len() as u32);
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
        test_support::{fixture_video_config, keyframe_annexb, mono_48k},
    };

    const DESTINATION: Destination = Destination {
        address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        video_port: 5_004,
        audio_port: 5_006,
        rtcp_mux: false,
    };

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
            .push(&MediaEvent::VideoConfig(fixture_video_config()))
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
        let description = muxer.session_description(&DESTINATION).unwrap();

        // Assert: both streams share one cname
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
    fn reports_each_stream_at_the_send_time() {
        // Arrange: video at 1.0 s and 1.5 s, audio at 1.0 s, captured from 10 000 s
        let mut muxer = Muxer::new(MuxerOptions {
            video_ssrc: Some(1),
            audio_ssrc: Some(2),
            ..MuxerOptions::default()
        });
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();
        let video = |millis| {
            MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::from_millis(millis),
                dts: Timestamp::from_millis(millis),
            })
        };
        let first_video = headers(&muxer.push(&video(1_000)).unwrap());
        let last_video = headers(&muxer.push(&video(1_500)).unwrap());
        let audio = headers(
            &muxer
                .push(&MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[0x21, 0x10]),
                    pts: Timestamp::from_millis(1_000),
                }))
                .unwrap(),
        );

        // Act
        let reports = muxer.sender_reports(
            Timestamp::from_micros(10_000_000_000),
            Timestamp::from_micros(10_002_000_000),
        );

        // Assert: both report 10 002 s after the Unix epoch, the RTP times 0.5 s and 1.0 s past their latest samples; the packet count covers every video packet
        let field = |report: &RtcpPacket, at: usize| {
            u32::from_be_bytes(report.data[at..at + 4].try_into().unwrap())
        };
        let ntp_seconds = |report: &RtcpPacket| field(report, 8) as u64 - 2_208_988_800;
        assert_eq!(reports.len(), 2);
        assert_eq!(
            (reports[0].kind, field(&reports[0], 4)),
            (MediaKind::Video, 1)
        );
        assert_eq!(ntp_seconds(&reports[0]), 10_002);
        assert_eq!(field(&reports[0], 12), 0);
        assert_eq!(
            field(&reports[0], 16),
            last_video[0].timestamp.wrapping_add(45_000)
        );
        assert_eq!(field(&reports[0], 20), (first_video.len() * 2) as u32);
        assert_eq!(
            (reports[1].kind, field(&reports[1], 4)),
            (MediaKind::Audio, 2)
        );
        assert_eq!(ntp_seconds(&reports[1]), 10_002);
        assert_eq!(
            field(&reports[1], 16),
            audio[0].timestamp.wrapping_add(48_000)
        );
    }

    #[test]
    fn sends_rtcp_to_the_port_above_or_the_rtp_port_when_muxed() {
        // Arrange
        let muxed = Destination {
            rtcp_mux: true,
            ..DESTINATION
        };

        // Act / Assert
        assert_eq!(DESTINATION.rtcp(MediaKind::Video).port(), 5_005);
        assert_eq!(DESTINATION.rtcp(MediaKind::Audio).port(), 5_007);
        assert_eq!(muxed.rtcp(MediaKind::Audio).port(), 5_006);
        assert_eq!(muxed.rtp(MediaKind::Video).port(), 5_004);
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
                .session_description(&DESTINATION)
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
            .push(&MediaEvent::VideoConfig(fixture_video_config()))
            .unwrap();

        // Act
        let without_audio = muxer.session_description(&DESTINATION);
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();
        let description = muxer.session_description(&DESTINATION).unwrap();

        // Assert
        assert!(without_audio.is_none());
        assert_eq!(description.destination, Some(DESTINATION.address));
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
            .push(&MediaEvent::VideoConfig(fixture_video_config()))
            .unwrap();

        // Act
        let description = muxer.session_description(&DESTINATION).unwrap();

        // Assert
        assert_eq!(description.media.len(), 1);
        assert_eq!(description.media[0].kind, MediaKind::Video);
    }
}
