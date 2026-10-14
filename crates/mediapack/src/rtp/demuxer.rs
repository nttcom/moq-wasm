use std::{collections::BTreeMap, mem};

use anyhow::{Result, ensure};
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig,
    h264::{AvcDecoderConfigurationRecord, ParameterSetTracker, annexb::with_start_codes},
    rtp::{
        aac, h264,
        packet::{Header, extend_wrapping},
        reorder::{Ordered, ReorderBuffer},
        sdp::{Codec, OPUS_CLOCK_RATE, SessionDescription},
    },
    sample::{AudioConfig, AudioSample, MediaEvent, StreamSet, Timestamp, VideoSample},
};

/// Turns the RTP datagrams of a session into media events. The session
/// description names the streams; a datagram whose payload type it does not
/// list, such as RTCP on a shared port, is ignored.
pub struct Demuxer {
    streams: BTreeMap<u8, Stream>,
    /// Emitted with the first datagram, as the other demuxers announce the
    /// streams when they first see them.
    announcement: Vec<MediaEvent>,
    discontinuities: u64,
}

struct Stream {
    clock_rate: u32,
    reorder: ReorderBuffer,
    clock: StreamClock,
    codec: StreamCodec,
}

enum StreamCodec {
    H264(VideoStream),
    Aac(AudioStream),
    /// A-law and Opus: every packet carries whole frames (RFC 3551 §4.5.14,
    /// RFC 7587 §4.2), so a loss costs only the packets lost.
    WholeFrames,
}

struct VideoStream {
    depacketizer: h264::Depacketizer,
    parameter_sets: ParameterSetTracker,
    /// NAL units of the access unit being assembled, which ends at the
    /// marker bit or when a packet with another timestamp arrives.
    access_unit: Vec<Bytes>,
    access_unit_timestamp: Option<u32>,
    access_unit_pts: Timestamp,
    /// A packet of this access unit was lost, so it is discarded when complete.
    corrupt: bool,
    /// Frames predicted from a lost one are withheld until a keyframe starts
    /// a decodable sequence again.
    awaiting_keyframe: bool,
}

struct AudioStream {
    depacketizer: aac::Depacketizer,
    frame_duration: Timestamp,
}

/// Extends the 32-bit RTP timestamp past its wrap and measures it from the
/// first packet of the stream.
#[derive(Default)]
struct StreamClock {
    origin: Option<u64>,
    highest: Option<u64>,
}

impl Demuxer {
    /// Fails when the description has no H.264, AAC, A-law or Opus stream, or
    /// gives two streams the same payload type.
    pub fn new(description: &SessionDescription) -> Result<Self> {
        let mut streams = BTreeMap::new();
        let mut set = StreamSet::default();
        let mut announcement = Vec::new();
        for media in &description.media {
            let codec = match &media.codec {
                Codec::H264 { parameter_sets } => {
                    set.has_video = true;
                    if let Some(record) = parameter_sets {
                        announcement.push(MediaEvent::VideoConfig(record.clone()));
                    }
                    StreamCodec::H264(VideoStream::new(parameter_sets.as_ref()))
                }
                Codec::Aac { config, au_headers } => {
                    set.has_audio = true;
                    announcement.push(MediaEvent::AudioConfig(AudioConfig::Aac(config.clone())));
                    StreamCodec::Aac(AudioStream::new(config, *au_headers))
                }
                Codec::Pcma { channels } => {
                    set.has_audio = true;
                    announcement.push(MediaEvent::AudioConfig(AudioConfig::Pcma {
                        sample_rate: media.clock_rate,
                        channels: *channels,
                    }));
                    StreamCodec::WholeFrames
                }
                Codec::Opus { channels } => {
                    set.has_audio = true;
                    announcement.push(MediaEvent::AudioConfig(AudioConfig::Opus {
                        sample_rate: OPUS_CLOCK_RATE,
                        channels: *channels,
                    }));
                    StreamCodec::WholeFrames
                }
                Codec::Unsupported(_) => continue,
            };
            ensure!(
                !streams.contains_key(&media.payload_type),
                "payload type {} describes two streams",
                media.payload_type
            );
            streams.insert(
                media.payload_type,
                Stream {
                    clock_rate: media.clock_rate,
                    reorder: ReorderBuffer::new(media.clock_rate),
                    clock: StreamClock::default(),
                    codec,
                },
            );
        }
        ensure!(
            !streams.is_empty(),
            "session description has no H.264, AAC, A-law or Opus stream"
        );
        announcement.insert(0, MediaEvent::Streams(set));
        Ok(Self {
            streams,
            announcement,
            discontinuities: 0,
        })
    }

    /// Fails when `datagram` is not a well-formed RTP packet or its payload
    /// breaks the stream's payload format.
    pub fn push(&mut self, datagram: &[u8]) -> Result<Vec<MediaEvent>> {
        let mut events = mem::take(&mut self.announcement);
        let (header, payload) = Header::parse(datagram)?;
        let Some(stream) = self.streams.get_mut(&header.payload_type) else {
            return Ok(events);
        };
        for item in stream.reorder.push(header, Bytes::copy_from_slice(payload)) {
            self.discontinuities += stream.handle(item, &mut events)?;
        }
        Ok(events)
    }

    /// How many times packets were found missing.
    pub fn discontinuities(&self) -> u64 {
        self.discontinuities
    }

    /// Releases the packets held for reordering and the open access unit.
    pub fn finish(&mut self) -> Result<Vec<MediaEvent>> {
        let mut events = mem::take(&mut self.announcement);
        for stream in self.streams.values_mut() {
            for item in stream.reorder.finish() {
                self.discontinuities += stream.handle(item, &mut events)?;
            }
            if let StreamCodec::H264(video) = &mut stream.codec {
                video.complete(&mut events)?;
            }
        }
        Ok(events)
    }
}

impl Stream {
    /// Returns 1 when the item reports lost packets, 0 otherwise.
    fn handle(&mut self, item: Ordered, events: &mut Vec<MediaEvent>) -> Result<u64> {
        match item {
            Ordered::Lost => {
                match &mut self.codec {
                    StreamCodec::H264(video) => {
                        video.depacketizer.reset();
                        video.corrupt = true;
                    }
                    StreamCodec::Aac(audio) => audio.depacketizer.reset(),
                    StreamCodec::WholeFrames => {}
                }
                Ok(1)
            }
            Ordered::Packet(header, payload) => {
                let ticks = self.clock.elapsed_ticks(header.timestamp);
                let pts = Timestamp::from_ticks(ticks, self.clock_rate);
                match &mut self.codec {
                    StreamCodec::H264(video) => video.packet(&header, &payload, pts, events)?,
                    StreamCodec::Aac(audio) => audio.packet(&header, &payload, pts, events)?,
                    StreamCodec::WholeFrames => {
                        events.push(MediaEvent::Audio(AudioSample { data: payload, pts }))
                    }
                }
                Ok(0)
            }
        }
    }
}

impl VideoStream {
    fn new(parameter_sets: Option<&AvcDecoderConfigurationRecord>) -> Self {
        Self {
            depacketizer: h264::Depacketizer::new(),
            parameter_sets: parameter_sets
                .map_or_else(ParameterSetTracker::new, ParameterSetTracker::from_record),
            access_unit: Vec::new(),
            access_unit_timestamp: None,
            access_unit_pts: Timestamp::ZERO,
            corrupt: false,
            awaiting_keyframe: false,
        }
    }

    fn packet(
        &mut self,
        header: &Header,
        payload: &[u8],
        pts: Timestamp,
        events: &mut Vec<MediaEvent>,
    ) -> Result<()> {
        if self
            .access_unit_timestamp
            .is_some_and(|timestamp| timestamp != header.timestamp)
        {
            self.complete(events)?;
        }
        self.access_unit_timestamp = Some(header.timestamp);
        self.access_unit_pts = pts;
        let nals = self.depacketizer.push(payload)?;
        self.access_unit.extend(nals);
        if header.marker {
            self.complete(events)?;
        }
        Ok(())
    }

    fn complete(&mut self, events: &mut Vec<MediaEvent>) -> Result<()> {
        let nals = mem::take(&mut self.access_unit);
        self.access_unit_timestamp = None;
        if nals.is_empty() {
            return Ok(());
        }
        if mem::take(&mut self.corrupt) {
            self.awaiting_keyframe = true;
            return Ok(());
        }
        let annexb = with_start_codes(nals.iter().map(Bytes::as_ref));
        let Some(unit) = self.parameter_sets.track(&annexb)? else {
            return Ok(());
        };
        if let Some(config) = unit.config_changed {
            events.push(MediaEvent::VideoConfig(config));
        }
        if self.awaiting_keyframe && !unit.is_keyframe {
            return Ok(());
        }
        self.awaiting_keyframe = false;
        events.push(MediaEvent::Video(VideoSample {
            data: unit.data,
            is_keyframe: unit.is_keyframe,
            pts: self.access_unit_pts,
            dts: self.access_unit_pts,
        }));
        Ok(())
    }
}

impl AudioStream {
    fn new(config: &AudioSpecificConfig, au_headers: aac::AuHeaderConfig) -> Self {
        Self {
            depacketizer: aac::Depacketizer::new(au_headers),
            frame_duration: config.frame_duration(),
        }
    }

    fn packet(
        &mut self,
        header: &Header,
        payload: &[u8],
        pts: Timestamp,
        events: &mut Vec<MediaEvent>,
    ) -> Result<()> {
        for unit in self.depacketizer.push(payload, header.marker)? {
            let offset = Timestamp::from_micros(self.frame_duration.micros() * unit.frame_offset);
            events.push(MediaEvent::Audio(AudioSample {
                data: unit.data,
                pts: pts.saturating_add(offset),
            }));
        }
        Ok(())
    }
}

impl StreamClock {
    fn elapsed_ticks(&mut self, timestamp: u32) -> u64 {
        let extended = extend_wrapping(&mut self.highest, timestamp as u64, 32);
        let origin = *self.origin.get_or_insert(extended);
        extended.saturating_sub(origin)
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::*;
    use crate::{
        rtp::muxer::{Muxer, RtpPacket},
        test_support::{
            FIXTURE_PPS, FIXTURE_SPS, audio_samples, delta_frame_annexb, fixture_record,
            keyframe_annexb, mono_48k, video_samples,
        },
    };

    const DESTINATION: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    fn source_events() -> Vec<MediaEvent> {
        vec![
            MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true,
            }),
            MediaEvent::VideoConfig(fixture_record()),
            MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())),
            MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::from_millis(1_000),
                dts: Timestamp::from_millis(1_000),
            }),
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[0x21, 0x10, 0x04]),
                pts: Timestamp::from_millis(1_000),
            }),
            MediaEvent::Video(VideoSample {
                data: delta_frame_annexb(),
                is_keyframe: false,
                pts: Timestamp::from_millis(1_033),
                dts: Timestamp::from_millis(1_033),
            }),
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[0xDE, 0x02]),
                pts: Timestamp::from_millis(1_021),
            }),
            MediaEvent::Video(VideoSample {
                data: keyframe_annexb(),
                is_keyframe: true,
                pts: Timestamp::from_millis(1_066),
                dts: Timestamp::from_millis(1_066),
            }),
        ]
    }

    fn mux(events: &[MediaEvent]) -> (Vec<RtpPacket>, SessionDescription) {
        let mut muxer = Muxer::default();
        let packets = events
            .iter()
            .flat_map(|event| muxer.push(event).unwrap())
            .collect();
        let description = muxer
            .session_description(DESTINATION, 5_004, 5_006)
            .unwrap();
        (packets, description)
    }

    fn demux(packets: &[RtpPacket], description: &SessionDescription) -> (Vec<MediaEvent>, u64) {
        let mut demuxer = Demuxer::new(description).unwrap();
        let mut events: Vec<MediaEvent> = packets
            .iter()
            .flat_map(|packet| demuxer.push(&packet.data).unwrap())
            .collect();
        events.extend(demuxer.finish().unwrap());
        (events, demuxer.discontinuities())
    }

    #[test]
    fn round_trips_video_and_audio_through_the_muxer() {
        // Arrange
        let (packets, description) = mux(&source_events());

        // Act
        let (events, discontinuities) = demux(&packets, &description);

        // Assert: timestamps restart at the first packet of each stream
        assert_eq!(discontinuities, 0);
        assert_eq!(
            &events[..3],
            &[
                MediaEvent::Streams(StreamSet {
                    has_video: true,
                    has_audio: true
                }),
                MediaEvent::VideoConfig(fixture_record()),
                MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())),
            ]
        );
        let video = video_samples(&events);
        assert_eq!(video.len(), 3);
        assert_eq!(video[0].data, keyframe_annexb());
        assert!(video[0].is_keyframe);
        assert_eq!(video[0].pts, Timestamp::ZERO);
        assert_eq!(video[1].data, delta_frame_annexb());
        assert_eq!(video[1].pts, Timestamp::from_millis(33));
        assert_eq!(video[2].pts, Timestamp::from_millis(66));
        let audio = audio_samples(&events);
        assert_eq!(audio.len(), 2);
        assert_eq!(audio[0].data.as_ref(), [0x21, 0x10, 0x04]);
        assert_eq!(audio[0].pts, Timestamp::ZERO);
        assert_eq!(audio[1].pts, Timestamp::from_millis(21));
    }

    #[test]
    fn reorders_packets_that_arrive_out_of_sequence() {
        // Arrange: the IDR arrives before the PPS of the same keyframe
        let (mut packets, description) = mux(&source_events());
        packets.swap(1, 2);

        // Act
        let (events, discontinuities) = demux(&packets, &description);

        // Assert
        assert_eq!(discontinuities, 0);
        assert_eq!(video_samples(&events)[0].data, keyframe_annexb());
    }

    #[test]
    fn drops_the_frames_cut_by_a_loss_until_the_next_keyframe() {
        // Arrange: the PPS packet of the first keyframe is lost
        let (mut packets, description) = mux(&source_events());
        let pps_index = packets
            .iter()
            .position(|packet| packet.data[12..] == FIXTURE_PPS)
            .unwrap();
        packets.remove(pps_index);

        // Act
        let (events, discontinuities) = demux(&packets, &description);

        // Assert: the first keyframe and the delta frame are gone, the second keyframe plays
        assert_eq!(discontinuities, 1);
        let video = video_samples(&events);
        assert_eq!(video.len(), 1);
        assert_eq!(video[0].pts, Timestamp::from_millis(66));
        assert_eq!(audio_samples(&events).len(), 2);
    }

    #[test]
    fn seeds_parameter_sets_from_the_session_description() {
        // Arrange: a sender that announces SPS/PPS only in the SDP
        let (packets, description) = mux(&source_events());
        let bare_keyframe: Vec<&RtpPacket> = packets
            .iter()
            .filter(|packet| packet.data[12..] != FIXTURE_SPS && packet.data[12..] != FIXTURE_PPS)
            .collect();
        let mut demuxer = Demuxer::new(&description).unwrap();

        // Act
        let events: Vec<MediaEvent> = bare_keyframe
            .iter()
            .flat_map(|packet| demuxer.push(&packet.data).unwrap())
            .collect();

        // Assert: the keyframe carries the parameter sets in band and the config is announced once
        assert_eq!(events[1], MediaEvent::VideoConfig(fixture_record()));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, MediaEvent::VideoConfig(_)))
                .count(),
            1
        );
        assert_eq!(video_samples(&events)[0].data, keyframe_annexb());
    }

    #[test]
    fn ignores_datagrams_of_other_payload_types() {
        // Arrange
        let (packets, description) = mux(&source_events());
        let mut demuxer = Demuxer::new(&description).unwrap();
        let mut rtcp_like = packets[0].data.to_vec();
        rtcp_like[1] = 0xC8;

        // Act
        let first = demuxer.push(&rtcp_like).unwrap();
        let second = demuxer.push(&rtcp_like).unwrap();

        // Assert: the announcement still comes with the first datagram
        assert_eq!(first.len(), 3);
        assert!(second.is_empty());
    }

    #[test]
    fn round_trips_a_law_and_opus_frames() {
        // Arrange
        let round_trip = |config: AudioConfig| {
            let source = [
                MediaEvent::Streams(StreamSet {
                    has_video: false,
                    has_audio: true,
                }),
                MediaEvent::AudioConfig(config),
                MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[0xD5; 160]),
                    pts: Timestamp::from_millis(500),
                }),
                MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[0x55; 160]),
                    pts: Timestamp::from_millis(520),
                }),
            ];
            let (packets, description) = mux(&source);
            demux(&packets, &description).0
        };

        // Act
        let pcma = round_trip(AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        });
        let opus = round_trip(AudioConfig::Opus {
            sample_rate: 48_000,
            channels: 2,
        });

        // Assert: the 20 ms between the frames survives on both clocks
        for (events, config) in [
            (
                &pcma,
                AudioConfig::Pcma {
                    sample_rate: 8_000,
                    channels: 1,
                },
            ),
            (
                &opus,
                AudioConfig::Opus {
                    sample_rate: 48_000,
                    channels: 2,
                },
            ),
        ] {
            assert_eq!(events[1], MediaEvent::AudioConfig(config));
            let audio = audio_samples(events);
            assert_eq!(audio.len(), 2);
            assert_eq!(audio[1].data.as_ref(), [0x55; 160]);
            assert_eq!(audio[1].pts, Timestamp::from_millis(20));
        }
    }

    #[test]
    fn rejects_descriptions_without_a_supported_stream() {
        // Arrange
        let description =
            SessionDescription::parse("m=audio 5006 RTP/AVP 9\na=rtpmap:9 G722/8000\n").unwrap();
        let (_, mut duplicated) = mux(&source_events());
        duplicated.media[1].payload_type = duplicated.media[0].payload_type;

        // Act / Assert
        assert!(Demuxer::new(&description).is_err());
        assert!(Demuxer::new(&duplicated).is_err());
    }
}
