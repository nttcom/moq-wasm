use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use bytes::{Bytes, BytesMut};

use crate::{
    mp4::{
        atom::{
            HEADER_LENGTH as ATOM_HEADER_LENGTH, atoms, find, full_atom, peek, read_u32, read_u64,
        },
        track::{AudioCodec, Media, Track, annexb_access_unit, read_track},
    },
    sample::{AudioSample, MediaEvent, StreamSet, Timestamp, VideoSample},
};

const TFHD_BASE_DATA_OFFSET: u32 = 0x01;
const TFHD_SAMPLE_DESCRIPTION_INDEX: u32 = 0x02;
const TFHD_DEFAULT_SAMPLE_DURATION: u32 = 0x08;
const TFHD_DEFAULT_SAMPLE_SIZE: u32 = 0x10;
const TFHD_DEFAULT_SAMPLE_FLAGS: u32 = 0x20;
const TRUN_DATA_OFFSET: u32 = 0x0001;
const TRUN_FIRST_SAMPLE_FLAGS: u32 = 0x0004;
const TRUN_SAMPLE_DURATION: u32 = 0x0100;
const TRUN_SAMPLE_SIZE: u32 = 0x0200;
const TRUN_SAMPLE_FLAGS: u32 = 0x0400;
const TRUN_COMPOSITION_OFFSET: u32 = 0x0800;
const SAMPLE_IS_NON_SYNC: u32 = 0x0001_0000;

#[derive(Default)]
pub struct Demuxer {
    buffer: BytesMut,
    tracks: BTreeMap<u32, Track>,
    pending: Option<Fragment>,
}

/// trun states the offset of its samples from the start of the enclosing moof,
/// so the fragment remembers how far the moof reached into the stream.
struct Fragment {
    runs: Vec<Run>,
    media_data_offset: usize,
}

struct Run {
    track_id: u32,
    data_offset: usize,
    decode_time: u64,
    samples: Vec<SampleEntry>,
}

struct SampleEntry {
    size: usize,
    duration: u64,
    composition_offset: i64,
    is_sync: bool,
}

impl Demuxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) -> Result<Vec<MediaEvent>> {
        self.buffer.extend_from_slice(data);
        let mut events = Vec::new();
        while let Some((size, kind)) = peek(&self.buffer).map(|(size, kind)| (size, kind.to_vec()))
        {
            if self.buffer.len() < size {
                break;
            }
            let atom = self.buffer.split_to(size).freeze();
            let payload = atom.slice(ATOM_HEADER_LENGTH..);
            match kind.as_slice() {
                b"moov" => events.extend(self.read_movie(&payload)?),
                b"moof" => {
                    ensure!(
                        self.pending.is_none(),
                        "fragment has no mdat before next moof"
                    );
                    self.pending = Some(read_fragment(&payload, size)?);
                }
                b"mdat" => events.extend(self.read_media_data(&payload)?),
                _ => {}
            }
        }
        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<MediaEvent>> {
        ensure!(self.buffer.is_empty(), "truncated MP4 atom at end of input");
        ensure!(
            self.pending.is_none(),
            "fragment has no mdat at end of input"
        );
        Ok(Vec::new())
    }

    /// `MediaEvent` carries AAC audio only, so a fragmented file with MP3 audio
    /// is rejected rather than demuxed without its sound.
    fn read_movie(&mut self, payload: &[u8]) -> Result<Vec<MediaEvent>> {
        for trak in atoms(payload).filter(|atom| atom.kind == b"trak") {
            if let Some((track_id, track)) = read_track(&trak)? {
                self.tracks.insert(track_id, track);
            }
        }
        ensure!(!self.tracks.is_empty(), "moov declares no supported track");

        let mut events = vec![MediaEvent::Streams(StreamSet {
            has_video: self.has(|media| matches!(media, Media::Video(_))),
            has_audio: self.has(|media| matches!(media, Media::Audio(_))),
        })];
        for track in self.tracks.values() {
            events.push(match &track.media {
                Media::Video(config) => MediaEvent::VideoConfig(config.clone()),
                Media::Audio(AudioCodec::Aac(config)) => MediaEvent::AudioConfig(config.clone()),
                Media::Audio(AudioCodec::Mp3 { .. }) => {
                    anyhow::bail!("MP3 audio is not supported by the fragment demuxer")
                }
            });
        }
        Ok(events)
    }

    fn has(&self, predicate: impl Fn(&Media) -> bool) -> bool {
        self.tracks.values().any(|track| predicate(&track.media))
    }

    fn read_media_data(&mut self, payload: &[u8]) -> Result<Vec<MediaEvent>> {
        let Some(fragment) = self.pending.take() else {
            return Ok(Vec::new());
        };

        let mut events = Vec::new();
        for run in fragment.runs {
            let track = self
                .tracks
                .get(&run.track_id)
                .with_context(|| format!("fragment references unknown track {}", run.track_id))?;
            let mut decode_time = run.decode_time;
            let mut offset = run
                .data_offset
                .checked_sub(fragment.media_data_offset)
                .context("trun data offset precedes mdat payload")?;
            for sample in &run.samples {
                let end = offset
                    .checked_add(sample.size)
                    .context("sample offset overflow")?;
                let data = payload
                    .get(offset..end)
                    .with_context(|| format!("mdat is shorter than sample at offset {offset}"))?;
                events.push(sample.build_event(
                    track,
                    Bytes::copy_from_slice(data),
                    decode_time,
                )?);
                offset = end;
                decode_time = decode_time
                    .checked_add(sample.duration)
                    .context("decode time overflow")?;
            }
        }
        Ok(events)
    }
}

impl SampleEntry {
    fn build_event(&self, track: &Track, data: Bytes, decode_time: u64) -> Result<MediaEvent> {
        let dts = Timestamp::from_ticks(decode_time, track.timescale);
        let pts = shift(dts, self.composition_offset, track.timescale);
        Ok(match &track.media {
            Media::Video(config) => MediaEvent::Video(VideoSample {
                data: annexb_access_unit(config, &data, self.is_sync)?,
                is_keyframe: self.is_sync,
                pts,
                dts,
            }),
            Media::Audio(_) => MediaEvent::Audio(AudioSample { data, pts }),
        })
    }
}

fn shift(dts: Timestamp, composition_offset: i64, timescale: u32) -> Timestamp {
    let offset = Timestamp::from_ticks(composition_offset.unsigned_abs(), timescale);
    if composition_offset < 0 {
        dts.saturating_sub(offset)
    } else {
        dts.saturating_add(offset)
    }
}

fn read_fragment(payload: &[u8], moof_size: usize) -> Result<Fragment> {
    let mut runs = Vec::new();
    for traf in atoms(payload).filter(|atom| atom.kind == b"traf") {
        runs.extend(read_track_fragment(traf.payload)?);
    }
    Ok(Fragment {
        runs,
        media_data_offset: moof_size + ATOM_HEADER_LENGTH,
    })
}

fn read_track_fragment(payload: &[u8]) -> Result<Vec<Run>> {
    let tfhd = find(payload, b"tfhd").context("traf has no tfhd")?;
    let header = full_atom(tfhd.payload, "tfhd")?;
    let track_id = read_u32(header.body, 0)?;
    let mut offset = 4;
    ensure!(
        header.flags & TFHD_BASE_DATA_OFFSET == 0,
        "absolute fragment base data offsets are unsupported"
    );
    if header.flags & TFHD_SAMPLE_DESCRIPTION_INDEX != 0 {
        offset += 4;
    }
    let default_duration = optional_u32(
        header.body,
        &mut offset,
        header.flags,
        TFHD_DEFAULT_SAMPLE_DURATION,
    )?;
    let default_size = optional_u32(
        header.body,
        &mut offset,
        header.flags,
        TFHD_DEFAULT_SAMPLE_SIZE,
    )?;
    let default_flags = optional_u32(
        header.body,
        &mut offset,
        header.flags,
        TFHD_DEFAULT_SAMPLE_FLAGS,
    )?;

    let decode_time = match find(payload, b"tfdt") {
        Some(tfdt) => {
            let header = full_atom(tfdt.payload, "tfdt")?;
            if header.is_version_one() {
                read_u64(header.body, 0)?
            } else {
                read_u32(header.body, 0)? as u64
            }
        }
        None => 0,
    };

    let defaults = RunDefaults {
        track_id,
        duration: default_duration,
        size: default_size,
        flags: default_flags,
    };
    let mut runs = Vec::new();
    let mut decode_time = decode_time;
    for trun in atoms(payload).filter(|atom| atom.kind == b"trun") {
        let run = read_run(trun.payload, &defaults, decode_time)?;
        for sample in &run.samples {
            decode_time = decode_time
                .checked_add(sample.duration)
                .context("decode time overflow")?;
        }
        runs.push(run);
    }
    Ok(runs)
}

struct RunDefaults {
    track_id: u32,
    duration: Option<u32>,
    size: Option<u32>,
    flags: Option<u32>,
}

fn read_run(payload: &[u8], defaults: &RunDefaults, decode_time: u64) -> Result<Run> {
    let header = full_atom(payload, "trun")?;
    let sample_count = read_u32(header.body, 0)? as usize;
    let mut offset = 4;
    let data_offset = optional_u32(header.body, &mut offset, header.flags, TRUN_DATA_OFFSET)?;
    let first_sample_flags = optional_u32(
        header.body,
        &mut offset,
        header.flags,
        TRUN_FIRST_SAMPLE_FLAGS,
    )?;

    let mut samples = Vec::new();
    for index in 0..sample_count {
        let duration = optional_u32(header.body, &mut offset, header.flags, TRUN_SAMPLE_DURATION)?
            .or(defaults.duration)
            .unwrap_or(0);
        let size = optional_u32(header.body, &mut offset, header.flags, TRUN_SAMPLE_SIZE)?
            .or(defaults.size)
            .context("trun states no sample size")?;
        let flags = optional_u32(header.body, &mut offset, header.flags, TRUN_SAMPLE_FLAGS)?
            .or_else(|| (index == 0).then_some(first_sample_flags).flatten())
            .or(defaults.flags)
            .unwrap_or(0);
        let composition_offset = optional_u32(
            header.body,
            &mut offset,
            header.flags,
            TRUN_COMPOSITION_OFFSET,
        )?
        .map_or(0, |value| {
            if header.is_version_one() {
                value as i32 as i64
            } else {
                value as i64
            }
        });
        samples.push(SampleEntry {
            size: size as usize,
            duration: duration as u64,
            composition_offset,
            is_sync: flags & SAMPLE_IS_NON_SYNC == 0,
        });
    }

    Ok(Run {
        track_id: defaults.track_id,
        data_offset: usize::try_from(data_offset.context("trun has no data offset")? as i32)
            .context("negative trun data offsets are unsupported")?,
        decode_time,
        samples,
    })
}

fn optional_u32(body: &[u8], offset: &mut usize, flags: u32, wanted: u32) -> Result<Option<u32>> {
    if flags & wanted == 0 {
        return Ok(None);
    }
    let value = read_u32(body, *offset)?;
    *offset += 4;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mp4::Fmp4Muxer,
        test_support::{FIXTURE_MP4, FIXTURE_TS, audio_samples, count_events, video_samples},
    };

    fn demux(data: &[u8], chunk: usize) -> Vec<MediaEvent> {
        let mut demuxer = Demuxer::new();
        let mut events = Vec::new();
        for part in data.chunks(chunk) {
            events.extend(demuxer.push(part).unwrap());
        }
        events
    }

    #[test]
    fn demuxes_an_ffmpeg_written_fragmented_file() {
        // Arrange / Act
        let events = demux(FIXTURE_MP4, 997);

        // Assert
        assert_eq!(
            events[0],
            MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true
            })
        );
        assert_eq!(
            count_events(&events, |event| matches!(event, MediaEvent::VideoConfig(_))),
            1
        );
        assert_eq!(
            count_events(&events, |event| matches!(event, MediaEvent::AudioConfig(_))),
            1
        );
        let video = video_samples(&events);
        assert_eq!(video.len(), 9);
        assert_eq!(video.iter().filter(|sample| sample.is_keyframe).count(), 2);
        assert_eq!(audio_samples(&events).len(), 30);
        assert!(
            video
                .iter()
                .all(|sample| sample.data.starts_with(&[0, 0, 0, 1]))
        );
        assert!(video.windows(2).all(|pair| pair[0].dts < pair[1].dts));
    }

    #[test]
    fn reports_the_configuration_the_file_declares() {
        // Arrange
        let mut transport = crate::mpegts::Demuxer::new();
        let mut source = transport.push(FIXTURE_TS).unwrap();
        source.extend(transport.finish().unwrap());

        // Act
        let events = demux(FIXTURE_MP4, FIXTURE_MP4.len());

        // Assert
        let expected = source
            .iter()
            .find(|event| matches!(event, MediaEvent::VideoConfig(_)));
        let actual = events
            .iter()
            .find(|event| matches!(event, MediaEvent::VideoConfig(_)));
        assert_eq!(actual, expected);
    }

    #[test]
    fn round_trips_the_samples_the_muxer_wrote() {
        // Arrange
        let expected = demux(FIXTURE_MP4, FIXTURE_MP4.len());
        let mut muxer = Fmp4Muxer::new();
        let mut stream = BytesMut::new();
        for event in &expected {
            stream.extend_from_slice(&muxer.push(event).unwrap());
        }
        stream.extend_from_slice(&muxer.finish().unwrap());

        // Act
        let actual = demux(&stream, stream.len());

        // Assert
        let (expected_video, actual_video) = (video_samples(&expected), video_samples(&actual));
        assert_eq!(actual_video.len(), expected_video.len());
        for (index, (left, right)) in actual_video.iter().zip(&expected_video).enumerate() {
            assert_eq!(
                (index, left.is_keyframe, left.data.len()),
                (index, right.is_keyframe, right.data.len())
            );
            assert_eq!(left.data, right.data, "sample {index}");
            // Conversion through the muxer's 90 kHz timescale loses at most one tick.
            assert!(left.pts.micros().abs_diff(right.pts.micros()) <= 12);
            assert!(left.dts.micros().abs_diff(right.dts.micros()) <= 12);
        }
        assert_eq!(audio_samples(&actual).len(), audio_samples(&expected).len());
    }
    #[test]
    fn rejects_incomplete_input_on_finish() {
        // Arrange
        let mut demuxer = Demuxer::new();

        // Act
        demuxer.push(&FIXTURE_MP4[..FIXTURE_MP4.len() - 1]).unwrap();

        // Assert
        assert!(demuxer.finish().is_err());
    }
}
