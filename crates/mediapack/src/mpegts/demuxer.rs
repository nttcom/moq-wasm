use std::collections::HashMap;

use anyhow::Result;
use bytes::BytesMut;

use crate::{
    aac::{AdtsReader, AudioSpecificConfig},
    h264::ParameterSetTracker,
    mpegts::parser::{
        CLOCK_RATE, ElementaryStream, PAT_PID, PacketReader, STREAM_TYPE_AAC_ADTS,
        STREAM_TYPE_H264, TsPacket, parse_packet, parse_pat_pmt_pid, parse_pes_header, parse_pmt,
        parse_section_body,
    },
    sample::{AudioSample, MediaEvent, StreamSet, Timestamp, VideoSample},
};

#[derive(Default)]
pub struct Demuxer {
    packets: PacketReader,
    pmt_pid: Option<u16>,
    sections: HashMap<u16, BytesMut>,
    streams: HashMap<u16, PesStream>,
    video: VideoTrack,
    audio_config: Option<AudioSpecificConfig>,
    discontinuities: u64,
}

struct PesStream {
    kind: StreamKind,
    pending: BytesMut,
    adts: AdtsReader,
    continuity: Option<u8>,
    /// A packet went missing inside the PES packet being assembled, so what is
    /// pending is discarded at the next PES start instead of being emitted.
    corrupt: bool,
}

#[derive(Clone, Copy)]
enum StreamKind {
    H264,
    AacAdts,
}

#[derive(Default)]
struct VideoTrack {
    parameter_sets: ParameterSetTracker,
    last_pts: Timestamp,
    /// A frame was lost, so the frames that predict from it are withheld
    /// until a keyframe starts a decodable sequence again.
    awaiting_keyframe: bool,
}

impl Demuxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<MediaEvent>> {
        let mut events = Vec::new();
        for packet in self.packets.push(data) {
            let packet = parse_packet(&packet)?;
            if packet.transport_error {
                continue;
            }
            self.handle_packet(&packet, &mut events)?;
        }
        Ok(events)
    }

    /// How many times a continuity counter showed packets had gone missing.
    pub fn discontinuities(&self) -> u64 {
        self.discontinuities
    }

    pub fn finish(&mut self) -> Result<Vec<MediaEvent>> {
        let mut events = Vec::new();
        let mut pids: Vec<u16> = self.streams.keys().copied().collect();
        pids.sort_unstable();
        for pid in pids {
            self.flush_pes(pid, &mut events)?;
        }
        Ok(events)
    }

    fn handle_packet(&mut self, packet: &TsPacket, events: &mut Vec<MediaEvent>) -> Result<()> {
        if packet.pid == PAT_PID || Some(packet.pid) == self.pmt_pid {
            return self.handle_section_packet(packet, events);
        }
        let Some(stream) = self.streams.get_mut(&packet.pid) else {
            return Ok(());
        };
        if packet.payload.is_empty() {
            return Ok(());
        }
        match continuity_of(packet, stream.continuity) {
            Continuity::Duplicate => return Ok(()),
            Continuity::Broken => {
                stream.corrupt = true;
                self.discontinuities += 1;
            }
            Continuity::Intact => {}
        }
        stream.continuity = Some(packet.continuity_counter);
        if packet.payload_unit_start {
            self.flush_pes(packet.pid, events)?;
        }
        if let Some(stream) = self.streams.get_mut(&packet.pid) {
            stream.pending.extend_from_slice(packet.payload);
        }
        Ok(())
    }

    fn handle_section_packet(
        &mut self,
        packet: &TsPacket,
        events: &mut Vec<MediaEvent>,
    ) -> Result<()> {
        let buffer = self.sections.entry(packet.pid).or_default();
        if packet.payload_unit_start {
            buffer.clear();
            let Some((pointer, rest)) = packet.payload.split_first() else {
                return Ok(());
            };
            buffer.extend_from_slice(rest.get(*pointer as usize..).unwrap_or_default());
        } else {
            buffer.extend_from_slice(packet.payload);
        }
        let Some(body) = parse_section_body(buffer)? else {
            return Ok(());
        };
        if packet.pid == PAT_PID {
            self.pmt_pid = parse_pat_pmt_pid(body);
        } else {
            let streams = parse_pmt(body)?;
            self.register_streams(&streams, events);
        }
        self.sections.remove(&packet.pid);
        Ok(())
    }

    fn register_streams(&mut self, streams: &[ElementaryStream], events: &mut Vec<MediaEvent>) {
        let mut stream_set = StreamSet::default();
        for stream in streams {
            let kind = match stream.stream_type {
                STREAM_TYPE_H264 => {
                    stream_set.has_video = true;
                    StreamKind::H264
                }
                STREAM_TYPE_AAC_ADTS => {
                    stream_set.has_audio = true;
                    StreamKind::AacAdts
                }
                _ => continue,
            };
            self.streams.entry(stream.pid).or_insert_with(|| PesStream {
                continuity: None,
                corrupt: false,
                kind,
                pending: BytesMut::new(),
                adts: AdtsReader::new(),
            });
        }
        if !self.streams.is_empty() && !events.iter().any(|e| matches!(e, MediaEvent::Streams(_))) {
            events.push(MediaEvent::Streams(stream_set));
        }
    }

    fn flush_pes(&mut self, pid: u16, events: &mut Vec<MediaEvent>) -> Result<()> {
        let Some(stream) = self.streams.get_mut(&pid) else {
            return Ok(());
        };
        let pes = std::mem::take(&mut stream.pending);
        if std::mem::take(&mut stream.corrupt) {
            match stream.kind {
                StreamKind::H264 => self.video.awaiting_keyframe = true,
                StreamKind::AacAdts => stream.adts = AdtsReader::new(),
            }
            return Ok(());
        }
        let Some(header) = parse_pes_header(&pes)? else {
            return Ok(());
        };
        let payload = &pes[header.header_length..];
        let pts = header
            .pts
            .map(|ticks| Timestamp::from_ticks(ticks, CLOCK_RATE));
        let dts = header
            .dts
            .map(|ticks| Timestamp::from_ticks(ticks, CLOCK_RATE));
        match stream.kind {
            StreamKind::H264 => emit_video(&mut self.video, payload, pts, dts, events),
            StreamKind::AacAdts => emit_audio(
                &mut stream.adts,
                &mut self.audio_config,
                payload,
                pts,
                events,
            ),
        }
    }
}

enum Continuity {
    Intact,
    Duplicate,
    Broken,
}

/// ISO 13818-1 §2.4.3.3: the counter advances by one per packet with a
/// payload, a repeated value is a duplicate packet, and the discontinuity
/// indicator announces a legitimate jump.
fn continuity_of(packet: &TsPacket, last: Option<u8>) -> Continuity {
    let Some(last) = last else {
        return Continuity::Intact;
    };
    if packet.continuity_counter == last {
        return Continuity::Duplicate;
    }
    if packet.discontinuity || packet.continuity_counter == (last + 1) & 0x0F {
        return Continuity::Intact;
    }
    Continuity::Broken
}

fn emit_video(
    video: &mut VideoTrack,
    payload: &[u8],
    pts: Option<Timestamp>,
    dts: Option<Timestamp>,
    events: &mut Vec<MediaEvent>,
) -> Result<()> {
    let Some(unit) = video.parameter_sets.track(payload)? else {
        return Ok(());
    };
    if let Some(config) = unit.config_changed {
        events.push(MediaEvent::VideoConfig(config));
    }
    if video.awaiting_keyframe && !unit.is_keyframe {
        return Ok(());
    }
    video.awaiting_keyframe = false;
    let pts = pts.unwrap_or(video.last_pts);
    video.last_pts = pts;
    events.push(MediaEvent::Video(VideoSample {
        data: unit.data,
        is_keyframe: unit.is_keyframe,
        pts,
        dts: dts.unwrap_or(pts),
    }));
    Ok(())
}

fn emit_audio(
    adts: &mut AdtsReader,
    audio_config: &mut Option<AudioSpecificConfig>,
    payload: &[u8],
    pts: Option<Timestamp>,
    events: &mut Vec<MediaEvent>,
) -> Result<()> {
    let mut pts = pts.unwrap_or_default();
    for frame in adts.push(payload)? {
        if audio_config.as_ref() != Some(&frame.config) {
            events.push(MediaEvent::AudioConfig(frame.config.clone()));
            *audio_config = Some(frame.config.clone());
        }
        events.push(MediaEvent::Audio(AudioSample {
            data: frame.data,
            pts,
        }));
        pts = pts.saturating_add(frame.config.frame_duration());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        h264::{
            NalUnitType,
            annexb::{nal_units, with_start_codes},
            nal::nal_unit_type,
        },
        mpegts::parser::PACKET_SIZE,
        test_support::{
            FIXTURE_PPS, FIXTURE_SPS, FIXTURE_TS, IDR_SLICE, NON_IDR_SLICE, TsStreamBuilder,
            adts_frame, audio_samples, count_events, delta_frame_annexb, keyframe_annexb, mono_48k,
            pat_section, pes_packet, pmt_section, video_samples,
        },
    };
    use bytes::Bytes;

    const VIDEO_PID: u16 = 0x100;
    const AUDIO_PID: u16 = 0x101;

    fn hand_built_stream() -> TsStreamBuilder {
        let mut stream = TsStreamBuilder::default();
        stream.push(PAT_PID, &pat_section(0x1000));
        stream.push(
            0x1000,
            &pmt_section(&[
                (VIDEO_PID, STREAM_TYPE_H264),
                (AUDIO_PID, STREAM_TYPE_AAC_ADTS),
            ]),
        );
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 90_000, Some(86_400), &keyframe_annexb()),
        );
        let mut audio = adts_frame(&[1, 2, 3]);
        audio.extend(adts_frame(&[4, 5, 6]));
        stream.push(AUDIO_PID, &pes_packet(0xC0, 90_000, None, &audio));
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 93_000, None, &delta_frame_annexb()),
        );
        stream
    }

    /// A keyframe padded with a filler NAL unit so that its PES packet spans
    /// several transport packets.
    fn long_keyframe_annexb() -> Bytes {
        let filler = [&[0x0C][..], &[0xFF; 400]].concat();
        with_start_codes([&FIXTURE_SPS[..], &FIXTURE_PPS, &IDR_SLICE, &filler])
    }

    fn demux_all(stream: &[u8]) -> (Demuxer, Vec<MediaEvent>) {
        let mut demuxer = Demuxer::new();
        let mut events = demuxer.push(stream).unwrap();
        events.extend(demuxer.finish().unwrap());
        (demuxer, events)
    }

    #[test]
    fn demuxes_hand_built_program() {
        // Arrange
        let stream = hand_built_stream().build();

        // Act
        let (_, events) = demux_all(&stream);

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true
            })
        );
        let MediaEvent::VideoConfig(config) = &events[1] else {
            panic!("expected video config, got {:?}", events[1]);
        };
        assert_eq!(config.codec_string(), "avc1.42D00B");
        assert_eq!(config.sequence_parameter_set().unwrap().width, 160);
        assert!(events.contains(&MediaEvent::AudioConfig(mono_48k())));

        let video = video_samples(&events);
        assert_eq!(video.len(), 2);
        assert!(video[0].is_keyframe);
        assert_eq!(video[0].data, keyframe_annexb());
        assert_eq!(video[0].pts, Timestamp::from_micros(1_000_000));
        assert_eq!(video[0].dts, Timestamp::from_micros(960_000));
        assert!(!video[1].is_keyframe);
        assert_eq!(video[1].data, delta_frame_annexb());
        assert_eq!(video[1].dts, video[1].pts);

        let audio = audio_samples(&events);
        assert_eq!(audio.len(), 2);
        assert_eq!(audio[0].data.as_ref(), [1, 2, 3]);
        assert_eq!(audio[0].pts, Timestamp::from_micros(1_000_000));
        assert_eq!(audio[1].pts, Timestamp::from_micros(1_021_333));
    }

    #[test]
    fn prepends_known_parameter_sets_to_bare_keyframes() {
        // Arrange
        let mut stream = hand_built_stream();
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 96_000, None, &with_start_codes([&IDR_SLICE[..]])),
        );

        // Act
        let (_, events) = demux_all(&stream.build());

        // Assert
        let video = video_samples(&events);
        assert_eq!(video.len(), 3);
        assert!(video[2].is_keyframe);
        assert_eq!(video[2].data, keyframe_annexb());
    }

    #[test]
    fn ignores_pes_data_before_program_map() {
        // Arrange
        let orphan = TsStreamBuilder::default()
            .push(
                VIDEO_PID,
                &pes_packet(0xE0, 1, None, &with_start_codes([&NON_IDR_SLICE[..]])),
            )
            .build();
        let mut demuxer = Demuxer::new();

        // Act
        let events = demuxer.push(&orphan).unwrap();

        // Assert
        assert!(events.is_empty());
    }

    fn program_with_two_keyframes() -> TsStreamBuilder {
        let mut stream = TsStreamBuilder::default();
        stream.push(PAT_PID, &pat_section(0x1000));
        stream.push(0x1000, &pmt_section(&[(VIDEO_PID, STREAM_TYPE_H264)]));
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 90_000, None, &long_keyframe_annexb()),
        );
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 93_000, None, &delta_frame_annexb()),
        );
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 96_000, None, &long_keyframe_annexb()),
        );
        stream.push(
            VIDEO_PID,
            &pes_packet(0xE0, 99_000, None, &delta_frame_annexb()),
        );
        stream
    }

    #[test]
    fn drops_the_frame_cut_by_a_lost_packet_and_the_frames_predicted_from_it() {
        // Arrange: the second packet of the first keyframe never arrives
        let stream = program_with_two_keyframes();
        assert!(stream.packet_count() >= 8);
        let cut = stream.without_packet(3);

        // Act
        let (demuxer, events) = demux_all(&cut);

        // Assert
        let video = video_samples(&events);
        assert_eq!(video.len(), 2);
        assert!(video[0].is_keyframe);
        assert_eq!(video[0].pts, Timestamp::from_micros(1_066_666));
        assert!(!video[1].is_keyframe);
        assert_eq!(demuxer.discontinuities(), 1);
    }

    #[test]
    fn ignores_a_duplicated_packet() {
        // Arrange
        let stream = program_with_two_keyframes();
        let repeated = stream.with_packet_repeated(2);

        // Act
        let (demuxer, events) = demux_all(&repeated);

        // Assert
        assert_eq!(video_samples(&events).len(), 4);
        assert_eq!(demuxer.discontinuities(), 0);
    }

    #[test]
    fn demuxes_ffmpeg_generated_transport_stream_in_chunks() {
        // Arrange
        let mut demuxer = Demuxer::new();
        let mut events = Vec::new();

        // Act
        for chunk in FIXTURE_TS.chunks(PACKET_SIZE * 3 + 17) {
            events.extend(demuxer.push(chunk).unwrap());
        }
        events.extend(demuxer.finish().unwrap());

        // Assert
        let video = video_samples(&events);
        let audio = audio_samples(&events);
        assert_eq!(video.len(), 9);
        assert_eq!(video.iter().filter(|sample| sample.is_keyframe).count(), 2);
        assert_eq!(audio.len(), 30);
        assert_eq!(
            count_events(&events, |event| matches!(event, MediaEvent::VideoConfig(_))),
            1
        );
        assert_eq!(video[0].pts, Timestamp::from_ticks(127_920, CLOCK_RATE));
        assert!(video.windows(2).all(|pair| pair[0].dts < pair[1].dts));
        assert!(audio.windows(2).all(|pair| pair[0].pts < pair[1].pts));
        assert!(
            video
                .iter()
                .all(|sample| sample.data.starts_with(&[0, 0, 0, 1]))
        );
        assert!(video.iter().filter(|s| s.is_keyframe).all(|sample| {
            nal_units(&sample.data).any(|nal| nal_unit_type(nal) == Some(NalUnitType::Sps))
        }));
    }
}
