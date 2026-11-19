use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
};

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
    sample::{
        AudioConfig, AudioSample, MediaEvent, StreamSet, Timestamp, VideoConfig, VideoSample,
    },
};

/// Turns the RTP datagrams of a session into media events. The session
/// description names the streams; a datagram whose payload type it does not
/// list, such as RTCP on a shared port, is ignored. Each stream keeps the SSRC
/// of its first packet. A sender that restarts picks a new SSRC, sequence
/// numbers and timestamps, which cannot be ordered against the old ones, so
/// `push` drops its packets. Check each datagram with
/// [`Demuxer::is_new_source`] before `push`, and give a new sender's packets
/// to the demuxer [`Demuxer::for_next_source`] returns.
pub struct Demuxer {
    description: SessionDescription,
    streams: BTreeMap<u8, Stream>,
    /// Emitted with the first datagram, as the other demuxers announce the
    /// streams when they first see them.
    announcement: Vec<MediaEvent>,
    discontinuities: u64,
    /// SSRCs of the senders before the current one. `push` drops their late
    /// packets so they cannot claim a stream of the current sender.
    retired: BTreeSet<u32>,
}

struct Stream {
    ssrc: Option<u32>,
    clock_rate: u32,
    reorder: ReorderBuffer,
    clock: StreamClock,
    codec: StreamCodec,
}

enum StreamCodec {
    H264(VideoStream),
    Aac(AudioStream),
    /// A-law and Opus: every packet decodes on its own (A-law is sample-based,
    /// RFC 3551 §4.5.14; one Opus packet per RTP packet, RFC 7587 §4.2), so a
    /// loss costs only the packets lost.
    WholeFrames,
}

/// Bounds on the access unit being assembled, so a sender that never sets the
/// marker bit nor changes the timestamp cannot grow it without limit.
const MAX_ACCESS_UNIT_NALS: usize = 8192;
const MAX_ACCESS_UNIT_LENGTH: usize = 16 * 1024 * 1024;

struct VideoStream {
    depacketizer: h264::Depacketizer,
    parameter_sets: ParameterSetTracker,
    /// NAL units of the access unit being assembled, which ends at the
    /// marker bit or when a packet with another timestamp arrives.
    access_unit: Vec<Bytes>,
    /// Total bytes of the NAL units in `access_unit`.
    access_unit_length: usize,
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
                        announcement
                            .push(MediaEvent::VideoConfig(VideoConfig::Avc(record.clone())));
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
                    ssrc: None,
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
            description: description.clone(),
            streams,
            announcement,
            discontinuities: 0,
            retired: BTreeSet::new(),
        })
    }

    /// Returns a new demuxer for the sender that restarted. It drops packets
    /// from every SSRC whose packets reached this demuxer or its predecessors,
    /// because packets of the old sender still queued on another port would
    /// otherwise claim a stream before the new sender's packets reach it.
    pub fn for_next_source(&self) -> Self {
        let mut next =
            Self::new(&self.description).expect("the description already built this demuxer");
        next.retired = self.retired.clone();
        next.retired
            .extend(self.streams.values().filter_map(|stream| stream.ssrc));
        next
    }

    /// Fails when `datagram` is not a well-formed RTP packet or its payload
    /// breaks the stream's payload format.
    pub fn push(&mut self, datagram: &[u8]) -> Result<Vec<MediaEvent>> {
        let mut events = mem::take(&mut self.announcement);
        let (header, payload) = Header::parse(datagram)?;
        let Some(stream) = self.streams.get_mut(&header.payload_type) else {
            return Ok(events);
        };
        if self.retired.contains(&header.ssrc)
            || *stream.ssrc.get_or_insert(header.ssrc) != header.ssrc
        {
            return Ok(events);
        }
        for item in stream.reorder.push(header, Bytes::copy_from_slice(payload)) {
            self.discontinuities += stream.handle(item, &mut events)?;
        }
        Ok(events)
    }

    /// Whether `datagram` has another SSRC than the first packet of its
    /// stream. False for a datagram that does not parse, has an unlisted
    /// payload type, comes before its stream's first packet, or carries an
    /// SSRC that [`Demuxer::for_next_source`] retired.
    pub fn is_new_source(&self, datagram: &[u8]) -> bool {
        let Ok((header, _)) = Header::parse(datagram) else {
            return false;
        };
        !self.retired.contains(&header.ssrc)
            && self
                .streams
                .get(&header.payload_type)
                .and_then(|stream| stream.ssrc)
                .is_some_and(|ssrc| ssrc != header.ssrc)
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
                    StreamCodec::WholeFrames => events.push(MediaEvent::Audio(AudioSample {
                        data: payload,
                        pts,
                        audio_level: None,
                    })),
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
            access_unit_length: 0,
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
        self.access_unit_length += nals.iter().map(Bytes::len).sum::<usize>();
        self.access_unit.extend(nals);
        if self.access_unit.len() > MAX_ACCESS_UNIT_NALS
            || self.access_unit_length > MAX_ACCESS_UNIT_LENGTH
        {
            self.access_unit.clear();
            self.access_unit_length = 0;
            self.depacketizer.reset();
            self.corrupt = true;
        }
        if header.marker {
            self.complete(events)?;
        }
        Ok(())
    }

    fn complete(&mut self, events: &mut Vec<MediaEvent>) -> Result<()> {
        let nals = mem::take(&mut self.access_unit);
        self.access_unit_length = 0;
        self.access_unit_timestamp = None;
        if mem::take(&mut self.corrupt) {
            self.awaiting_keyframe = true;
            return Ok(());
        }
        if nals.is_empty() {
            return Ok(());
        }
        let annexb = with_start_codes(nals.iter().map(Bytes::as_ref));
        let Some(unit) = self.parameter_sets.track(&annexb)? else {
            return Ok(());
        };
        if let Some(config) = unit.config_changed {
            events.push(MediaEvent::VideoConfig(VideoConfig::Avc(config)));
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
                audio_level: None,
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
        rtp::{
            muxer::{Destination, Muxer, MuxerOptions, StreamPacket},
            sdp::MediaKind,
        },
        test_support::{
            FIXTURE_PPS, FIXTURE_SPS, NON_IDR_SLICE, audio_samples, delta_frame_annexb,
            fixture_video_config, h264_aac_configs, keyframe_annexb, keyframe_sample,
            video_samples,
        },
    };

    const DESTINATION: Destination = Destination {
        address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        video_port: 5_004,
        audio_port: 5_006,
        rtcp_mux: false,
    };

    fn source_events() -> Vec<MediaEvent> {
        let mut events = h264_aac_configs();
        events.extend([
            MediaEvent::Video(keyframe_sample(Timestamp::from_millis(1_000))),
            MediaEvent::Audio(AudioSample {
                data: Bytes::from_static(&[0x21, 0x10, 0x04]),
                pts: Timestamp::from_millis(1_000),
                audio_level: None,
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
                audio_level: None,
            }),
            MediaEvent::Video(keyframe_sample(Timestamp::from_millis(1_066))),
        ]);
        events
    }

    fn mux(events: &[MediaEvent]) -> (Vec<StreamPacket>, SessionDescription) {
        let mut muxer = Muxer::default();
        let packets = events
            .iter()
            .flat_map(|event| muxer.push(event).unwrap())
            .collect();
        let description = muxer.session_description(&DESTINATION).unwrap();
        (packets, description)
    }

    fn demux(packets: &[StreamPacket], description: &SessionDescription) -> (Vec<MediaEvent>, u64) {
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
        assert_eq!(&events[..3], h264_aac_configs().as_slice());
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

    /// Muxed source packets preceded by one video access unit made of `count`
    /// packets carrying `payload`.
    fn after_an_unterminated_access_unit(
        payload: &[u8],
        count: usize,
    ) -> (Vec<StreamPacket>, SessionDescription) {
        let (packets, description) = mux(&source_events());
        let first_video = packets
            .iter()
            .find(|packet| packet.kind == MediaKind::Video)
            .unwrap();
        let (template, _) = Header::parse(&first_video.data).unwrap();
        let mut crafted: Vec<StreamPacket> = (1..=count)
            .rev()
            .map(|before| StreamPacket {
                kind: MediaKind::Video,
                data: Header {
                    marker: false,
                    sequence_number: template.sequence_number.wrapping_sub(before as u16),
                    timestamp: template.timestamp.wrapping_sub(3_000),
                    ..template
                }
                .encode(payload),
            })
            .collect();
        crafted.extend(packets);
        (crafted, description)
    }

    #[test]
    fn drops_an_access_unit_with_too_many_nal_units() {
        // Arrange
        let (packets, description) =
            after_an_unterminated_access_unit(&NON_IDR_SLICE, MAX_ACCESS_UNIT_NALS + 1);

        // Act
        let (events, discontinuities) = demux(&packets, &description);

        // Assert: only the three source frames play
        assert_eq!(discontinuities, 0);
        let video = video_samples(&events);
        assert_eq!(video.len(), 3);
        assert!(video[0].is_keyframe);
    }

    #[test]
    fn drops_an_access_unit_with_too_many_bytes() {
        // Arrange
        let nal = vec![NON_IDR_SLICE[0]; 1024 * 1024];
        let count = MAX_ACCESS_UNIT_LENGTH / nal.len() + 1;
        let (packets, description) = after_an_unterminated_access_unit(&nal, count);

        // Act
        let (events, discontinuities) = demux(&packets, &description);

        // Assert: only the three source frames play
        assert_eq!(discontinuities, 0);
        let video = video_samples(&events);
        assert_eq!(video.len(), 3);
        assert!(video[0].is_keyframe);
    }

    #[test]
    fn seeds_parameter_sets_from_the_session_description() {
        // Arrange: a sender that announces SPS/PPS only in the SDP
        let (packets, description) = mux(&source_events());
        let bare_keyframe: Vec<&StreamPacket> = packets
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
        assert_eq!(events[1], MediaEvent::VideoConfig(fixture_video_config()));
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
    fn leaves_a_restarted_sender_to_a_new_demuxer() {
        // Arrange: a second muxer plays the restarted sender, with SSRC 7 and its own random sequence numbers
        let (first_run, description) = mux(&source_events());
        let mut restarted = Muxer::new(MuxerOptions {
            video_ssrc: Some(7),
            ..MuxerOptions::default()
        });
        let second_run: Vec<StreamPacket> = source_events()
            .iter()
            .flat_map(|event| restarted.push(event).unwrap())
            .collect();
        let mut demuxer = Demuxer::new(&description).unwrap();
        for packet in &first_run {
            demuxer.push(&packet.data).unwrap();
        }

        let straggler = first_run
            .iter()
            .find(|packet| packet.kind == MediaKind::Audio)
            .unwrap();

        // Act: an audio packet of the old sender reaches the next demuxer first
        let detected = demuxer.is_new_source(&second_run[0].data);
        let mixed_in = demuxer.push(&second_run[0].data).unwrap();
        let mut next = demuxer.for_next_source();
        next.push(&straggler.data).unwrap();
        let mut events = Vec::new();
        let mut flagged_as_new = 0;
        for packet in &second_run {
            flagged_as_new += next.is_new_source(&packet.data) as usize;
            events.extend(next.push(&packet.data).unwrap());
        }

        // Assert: the straggler neither claims the audio stream nor reads as a new source
        assert!(!demuxer.is_new_source(&first_run[0].data));
        assert!(detected);
        assert!(mixed_in.is_empty());
        assert!(!next.is_new_source(&straggler.data));
        assert_eq!(flagged_as_new, 0);
        assert_eq!(video_samples(&events).len(), 3);
        assert_eq!(audio_samples(&events).len(), 2);
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
                    audio_level: None,
                }),
                MediaEvent::Audio(AudioSample {
                    data: Bytes::from_static(&[0x55; 160]),
                    pts: Timestamp::from_millis(520),
                    audio_level: None,
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

    mod properties {
        use bytes::Bytes;
        use proptest::{collection::vec, prelude::*};

        use super::{demux, mux};
        use crate::{
            aac::AudioSpecificConfig,
            h264::annexb::with_start_codes,
            rtp::{
                muxer::{MAX_PAYLOAD, StreamPacket, VIDEO_CLOCK_RATE},
                packet::{HEADER_LENGTH, Header},
                sdp::{MediaKind, SessionDescription},
            },
            sample::{AudioConfig, AudioSample, MediaEvent, StreamSet, Timestamp, VideoSample},
            test_support::{
                FIXTURE_PPS, FIXTURE_SPS, audio_samples, fixture_video_config, video_samples,
            },
        };

        const IDR_HEADER: u8 = 0x65;
        /// Non-IDR slice headers of each nal_ref_idc an FU-A indicator copies.
        const NON_IDR_HEADERS: [u8; 3] = [0x41, 0x21, 0x01];

        /// `length` bytes, none of them zero, so a NAL unit holds no start
        /// code and ends in no zero byte. Their 255-byte period does not
        /// divide a fragment, so fragments out of order read differently.
        fn pattern(length: usize, seed: u8) -> impl Iterator<Item = u8> {
            (0..length).map(move |index| ((seed as usize + index) % 255) as u8 + 1)
        }

        /// NAL unit lengths, header byte included, weighted toward the 1200-byte
        /// payload: whole units of 1197 to 1203 bytes, and FU-A units whose
        /// 1198-byte fragments leave the last one within 3 bytes of full.
        fn nal_length() -> impl Strategy<Value = usize> {
            prop_oneof![
                1..=16usize,
                1_197..=1_203usize,
                (1..=4usize, 0..=6usize)
                    .prop_map(|(fragments, delta)| 1 + 1_198 * fragments + delta - 3),
                1..=6_000usize,
            ]
        }

        /// AAC access unit lengths, weighted toward full 1196-byte fragments
        /// after the 4-byte AU header section, and toward the 8191 bytes the
        /// 13-bit AU-size holds.
        fn aac_length() -> impl Strategy<Value = usize> {
            prop_oneof![
                1..=16usize,
                (1..=6usize, 0..=6usize)
                    .prop_map(|(fragments, delta)| 1_196 * fragments + delta - 3),
                8_188..=8_191usize,
                1..=8_191usize,
            ]
        }

        /// Each sample with the data the demuxer plays it as: a keyframe sent
        /// without parameter sets gets those of the session description.
        fn video(start: u64) -> impl Strategy<Value = Vec<(VideoSample, Bytes)>> {
            let slice = (nal_length(), any::<u8>(), 0..NON_IDR_HEADERS.len());
            let frame = (
                1..=100_000u64,
                any::<bool>(),
                any::<bool>(),
                vec(slice, 1..4),
            );
            vec(frame, 1..12).prop_map(move |frames| {
                let mut pts = start;
                frames
                    .into_iter()
                    .map(|(step, is_keyframe, in_band, slices)| {
                        let slices: Vec<Vec<u8>> = slices
                            .into_iter()
                            .map(|(length, seed, reference)| {
                                let header = if is_keyframe {
                                    IDR_HEADER
                                } else {
                                    NON_IDR_HEADERS[reference]
                                };
                                std::iter::once(header)
                                    .chain(pattern(length - 1, seed))
                                    .collect()
                            })
                            .collect();
                        let slices = slices.iter().map(Vec::as_slice);
                        let parameter_sets = [&FIXTURE_SPS[..], &FIXTURE_PPS];
                        let played = if is_keyframe {
                            with_start_codes(parameter_sets.into_iter().chain(slices.clone()))
                        } else {
                            with_start_codes(slices.clone())
                        };
                        let sample = VideoSample {
                            data: if in_band {
                                played.clone()
                            } else {
                                with_start_codes(slices)
                            },
                            is_keyframe,
                            pts: Timestamp::from_micros(pts),
                            dts: Timestamp::from_micros(pts),
                        };
                        pts += step;
                        (sample, played)
                    })
                    .collect()
            })
        }

        fn audio(start: u64) -> impl Strategy<Value = Vec<AudioSample>> {
            vec((1..=100_000u64, aac_length(), any::<u8>()), 1..12).prop_map(move |frames| {
                let mut pts = start;
                frames
                    .into_iter()
                    .map(|(step, length, seed)| {
                        let sample = AudioSample {
                            data: pattern(length, seed).collect(),
                            pts: Timestamp::from_micros(pts),
                            audio_level: None,
                        };
                        pts += step;
                        sample
                    })
                    .collect()
            })
        }

        #[derive(Debug)]
        struct Media {
            video: Vec<(VideoSample, Bytes)>,
            audio: Vec<AudioSample>,
            audio_config: AudioSpecificConfig,
        }

        /// Starts reach past the 2^32 ticks of the 90 kHz clock, so the muxer
        /// truncates them to the 32-bit RTP timestamp.
        fn media() -> impl Strategy<Value = Media> {
            let sample_rate = prop_oneof![Just(48_000u32), Just(44_100), Just(16_000)];
            (0..100_000_000_000u64, sample_rate).prop_flat_map(|(start, sample_rate)| {
                (video(start), audio(start)).prop_map(move |(video, audio)| Media {
                    video,
                    audio,
                    audio_config: AudioSpecificConfig::new(2, sample_rate, 1),
                })
            })
        }

        /// The first sequence number and timestamp of a stream. Half of them
        /// lie just below the 16-bit and 32-bit wraps so the stream crosses
        /// them; uniform starts almost never do.
        fn stream_start() -> impl Strategy<Value = (u16, u32)> {
            (
                prop_oneof![u16::MAX - 64..=u16::MAX, any::<u16>()],
                prop_oneof![u32::MAX - 100_000..=u32::MAX, any::<u32>()],
            )
        }

        /// The muxer's packets with each stream moved to start at the given
        /// sequence number and timestamp, as if the muxer had drawn them.
        fn muxed(
            media: &Media,
            starts: [(u16, u32); 2],
        ) -> (Vec<StreamPacket>, SessionDescription) {
            let mut events = vec![
                MediaEvent::Streams(StreamSet {
                    has_video: true,
                    has_audio: true,
                }),
                MediaEvent::VideoConfig(fixture_video_config()),
                MediaEvent::AudioConfig(AudioConfig::Aac(media.audio_config.clone())),
            ];
            let mut samples: Vec<MediaEvent> = media
                .video
                .iter()
                .map(|(sample, _)| MediaEvent::Video(sample.clone()))
                .chain(media.audio.iter().cloned().map(MediaEvent::Audio))
                .collect();
            samples.sort_by_key(|event| match event {
                MediaEvent::Video(sample) => sample.pts,
                MediaEvent::Audio(sample) => sample.pts,
                _ => Timestamp::ZERO,
            });
            events.extend(samples);
            let (mut packets, description) = mux(&events);
            for (kind, (sequence_number, timestamp)) in
                [MediaKind::Video, MediaKind::Audio].into_iter().zip(starts)
            {
                let mut shift = None;
                for packet in packets.iter_mut().filter(|packet| packet.kind == kind) {
                    let (header, payload) = Header::parse(&packet.data).unwrap();
                    let (sequence_shift, timestamp_shift) = *shift.get_or_insert((
                        sequence_number.wrapping_sub(header.sequence_number),
                        timestamp.wrapping_sub(header.timestamp),
                    ));
                    packet.data = Header {
                        sequence_number: header.sequence_number.wrapping_add(sequence_shift),
                        timestamp: header.timestamp.wrapping_add(timestamp_shift),
                        ..header
                    }
                    .encode(payload);
                }
            }
            (packets, description)
        }

        /// `pts` as the demuxer reads it back: the whole ticks of `clock_rate`
        /// the muxer rounded it to, counted from the stream's first sample.
        fn on_rtp_clock(pts: Timestamp, first: Timestamp, clock_rate: u32) -> Timestamp {
            Timestamp::from_ticks(pts.ticks(clock_rate) - first.ticks(clock_rate), clock_rate)
        }

        fn expected_video(media: &Media) -> Vec<VideoSample> {
            let first = media.video[0].0.pts;
            media
                .video
                .iter()
                .map(|(sample, played)| {
                    let pts = on_rtp_clock(sample.pts, first, VIDEO_CLOCK_RATE);
                    VideoSample {
                        data: played.clone(),
                        is_keyframe: sample.is_keyframe,
                        pts,
                        dts: pts,
                    }
                })
                .collect()
        }

        fn expected_audio(media: &Media) -> Vec<AudioSample> {
            let first = media.audio[0].pts;
            let clock_rate = media.audio_config.sample_rate;
            media
                .audio
                .iter()
                .map(|sample| AudioSample {
                    pts: on_rtp_clock(sample.pts, first, clock_rate),
                    ..sample.clone()
                })
                .collect()
        }

        /// Swaps the packets of each stream in disjoint adjacent pairs, one
        /// flag per pair start. The first packet of a stream stays first: the
        /// demuxer starts the stream there and drops earlier packets as late.
        fn swap_adjacent(packets: Vec<StreamPacket>, swaps: &[bool]) -> Vec<StreamPacket> {
            [MediaKind::Video, MediaKind::Audio]
                .into_iter()
                .flat_map(|kind| {
                    let mut stream: Vec<StreamPacket> = packets
                        .iter()
                        .filter(|packet| packet.kind == kind)
                        .cloned()
                        .collect();
                    let mut index = 1;
                    while index + 1 < stream.len() {
                        if swaps.get(index).copied().unwrap_or(false) {
                            stream.swap(index, index + 1);
                            index += 2;
                        } else {
                            index += 1;
                        }
                    }
                    stream
                })
                .collect()
        }

        proptest! {
            #[test]
            fn demuxing_muxed_samples_gives_them_back(
                media in media(),
                video_start in stream_start(),
                audio_start in stream_start(),
            ) {
                // Arrange
                let (packets, description) = muxed(&media, [video_start, audio_start]);

                // Act
                let (events, discontinuities) = demux(&packets, &description);

                // Assert: every packet also fits the payload bound
                prop_assert!(packets.iter().all(|packet| packet.data.len() <= HEADER_LENGTH + MAX_PAYLOAD));
                prop_assert_eq!(discontinuities, 0);
                prop_assert_eq!(video_samples(&events).into_iter().cloned().collect::<Vec<_>>(), expected_video(&media));
                prop_assert_eq!(audio_samples(&events).into_iter().cloned().collect::<Vec<_>>(), expected_audio(&media));
            }

            #[test]
            fn demuxing_gives_the_samples_back_when_adjacent_packets_swap(
                media in media(),
                video_start in stream_start(),
                audio_start in stream_start(),
                swaps in vec(any::<bool>(), 0..256),
            ) {
                // Arrange
                let (packets, description) = muxed(&media, [video_start, audio_start]);
                let packets = swap_adjacent(packets, &swaps);

                // Act
                let (events, discontinuities) = demux(&packets, &description);

                // Assert
                prop_assert_eq!(discontinuities, 0);
                prop_assert_eq!(video_samples(&events).into_iter().cloned().collect::<Vec<_>>(), expected_video(&media));
                prop_assert_eq!(audio_samples(&events).into_iter().cloned().collect::<Vec<_>>(), expected_audio(&media));
            }
        }
    }
}
