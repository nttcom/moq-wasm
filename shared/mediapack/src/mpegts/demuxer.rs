use std::collections::HashMap;

use anyhow::Result;
use bytes::{Bytes, BytesMut};

use crate::{
    aac::{AdtsReader, AudioSpecificConfig},
    h264::{
        AvcDecoderConfigurationRecord, NalUnitType,
        annexb::{nal_units, with_start_codes},
        nal::nal_unit_type,
    },
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
}

struct PesStream {
    kind: StreamKind,
    pending: BytesMut,
    adts: AdtsReader,
}

#[derive(Clone, Copy)]
enum StreamKind {
    H264,
    AacAdts,
}

#[derive(Default)]
struct VideoTrack {
    sps: Vec<Bytes>,
    pps: Vec<Bytes>,
    config: Option<AvcDecoderConfigurationRecord>,
    last_pts: Timestamp,
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
        if !self.streams.contains_key(&packet.pid) {
            return Ok(());
        }
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

fn emit_video(
    video: &mut VideoTrack,
    payload: &[u8],
    pts: Option<Timestamp>,
    dts: Option<Timestamp>,
    events: &mut Vec<MediaEvent>,
) -> Result<()> {
    let nals: Vec<&[u8]> = nal_units(payload).collect();
    if nals.is_empty() {
        return Ok(());
    }
    let mut is_keyframe = false;
    let mut inline_sps = Vec::new();
    let mut inline_pps = Vec::new();
    for nal in &nals {
        match nal_unit_type(nal) {
            Some(NalUnitType::IdrSlice) => is_keyframe = true,
            Some(NalUnitType::Sps) => inline_sps.push(Bytes::copy_from_slice(nal)),
            Some(NalUnitType::Pps) => inline_pps.push(Bytes::copy_from_slice(nal)),
            _ => {}
        }
    }
    if !inline_sps.is_empty() {
        video.sps = inline_sps.clone();
    }
    if !inline_pps.is_empty() {
        video.pps = inline_pps.clone();
    }
    if !video.sps.is_empty() && !video.pps.is_empty() {
        let config = AvcDecoderConfigurationRecord::from_parameter_sets(
            video.sps.clone(),
            video.pps.clone(),
        )?;
        if video.config.as_ref() != Some(&config) {
            events.push(MediaEvent::VideoConfig(config.clone()));
            video.config = Some(config);
        }
    }

    let data = if is_keyframe && inline_sps.is_empty() {
        with_start_codes(
            video
                .sps
                .iter()
                .chain(video.pps.iter())
                .map(|set| set.as_ref())
                .chain(nals.iter().copied()),
        )
    } else {
        with_start_codes(nals)
    };
    let pts = pts.unwrap_or(video.last_pts);
    video.last_pts = pts;
    events.push(MediaEvent::Video(VideoSample {
        data,
        is_keyframe,
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
        mpegts::parser::PACKET_SIZE,
        test_support::{
            FIXTURE_TS, IDR_SLICE, NON_IDR_SLICE, adts_frame, audio_samples, count_events,
            delta_frame_annexb, keyframe_annexb, mono_48k, pat_section, pes_packet, pmt_section,
            ts_packets, video_samples,
        },
    };

    const VIDEO_PID: u16 = 0x100;
    const AUDIO_PID: u16 = 0x101;

    fn hand_built_stream() -> Vec<u8> {
        let mut stream = ts_packets(PAT_PID, &pat_section(0x1000));
        stream.extend(ts_packets(
            0x1000,
            &pmt_section(&[
                (VIDEO_PID, STREAM_TYPE_H264),
                (AUDIO_PID, STREAM_TYPE_AAC_ADTS),
            ]),
        ));
        stream.extend(ts_packets(
            VIDEO_PID,
            &pes_packet(0xE0, 90_000, Some(86_400), &keyframe_annexb()),
        ));
        let mut audio = adts_frame(&[1, 2, 3]);
        audio.extend(adts_frame(&[4, 5, 6]));
        stream.extend(ts_packets(
            AUDIO_PID,
            &pes_packet(0xC0, 90_000, None, &audio),
        ));
        stream.extend(ts_packets(
            VIDEO_PID,
            &pes_packet(0xE0, 93_000, None, &delta_frame_annexb()),
        ));
        stream
    }

    #[test]
    fn demuxes_hand_built_program() {
        // Arrange
        let stream = hand_built_stream();
        let mut demuxer = Demuxer::new();

        // Act
        let mut events = demuxer.push(&stream).unwrap();
        events.extend(demuxer.finish().unwrap());

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
        stream.extend(ts_packets(
            VIDEO_PID,
            &pes_packet(0xE0, 96_000, None, &with_start_codes([&IDR_SLICE[..]])),
        ));
        let mut demuxer = Demuxer::new();

        // Act
        let mut events = demuxer.push(&stream).unwrap();
        events.extend(demuxer.finish().unwrap());

        // Assert
        let video = video_samples(&events);
        assert_eq!(video.len(), 3);
        assert!(video[2].is_keyframe);
        assert_eq!(video[2].data, keyframe_annexb());
    }

    #[test]
    fn ignores_pes_data_before_program_map() {
        // Arrange
        let orphan = ts_packets(
            VIDEO_PID,
            &pes_packet(0xE0, 1, None, &with_start_codes([&NON_IDR_SLICE[..]])),
        );
        let mut demuxer = Demuxer::new();

        // Act
        let events = demuxer.push(&orphan).unwrap();

        // Assert
        assert!(events.is_empty());
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
