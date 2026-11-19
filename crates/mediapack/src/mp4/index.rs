use anyhow::{Context, Result, ensure};
use bytes::Bytes;

use crate::{
    h264::AvcDecoderConfigurationRecord,
    mp4::{
        atom::{
            HEADER_LENGTH as ATOM_HEADER_LENGTH, atoms, find, find_full_atom, full_atom, peek,
            read_u32, read_u64,
        },
        track::{Media, annexb_access_unit, find_path, read_timescale, read_track},
    },
    sample::{AudioConfig, Timestamp, VideoConfig},
};

const MICROS_PER_SECOND: i128 = 1_000_000;
const EMPTY_EDIT_MEDIA_TIME: i64 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SampleKind {
    Video,
    Audio,
}

/// `offset` and `size` locate the sample's bytes in the file; a video sample
/// is stored as AVCC and becomes an Annex B access unit through
/// [`Mp4Index::annexb_video_sample`], an audio sample is one raw frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedSample {
    pub kind: SampleKind,
    pub offset: u64,
    pub size: u32,
    pub dts: Timestamp,
    pub pts: Timestamp,
    pub is_sync: bool,
}

/// ISO/IEC 14496-12 §8.6 and §8.7: a progressive file describes its samples
/// in the `stbl` tables of each track. The index lays the samples of the
/// first video and the first audio track out in decode order, with the edit
/// list applied and rebased so that the earliest sample starts at zero.
pub struct Mp4Index {
    pub video: Option<AvcDecoderConfigurationRecord>,
    pub audio: Option<AudioConfig>,
    samples: Vec<IndexedSample>,
    duration: Timestamp,
}

impl Mp4Index {
    pub fn parse(moov: &[u8]) -> Result<Self> {
        let (size, kind) = peek(moov).context("moov atom header is truncated")?;
        ensure!(kind == b"moov", "expected a moov atom");
        ensure!(size <= moov.len(), "moov atom is truncated");
        let payload = &moov[ATOM_HEADER_LENGTH..size];
        let movie_timescale = read_movie_timescale(payload)?;

        let mut video = None;
        let mut audio = None;
        let mut samples = Vec::new();
        for trak in atoms(payload).filter(|atom| atom.kind == b"trak") {
            let Some((_, track)) = read_track(&trak)? else {
                continue;
            };
            let kind = match track.media {
                Media::Video(VideoConfig::Avc(config)) if video.is_none() => {
                    video = Some(config);
                    SampleKind::Video
                }
                Media::Audio(codec) if audio.is_none() => {
                    audio = Some(codec);
                    SampleKind::Audio
                }
                _ => continue,
            };
            let stbl = find_path(trak.payload, &[b"mdia", b"minf", b"stbl"])
                .context("trak has no stbl")?;
            let table = read_sample_table(stbl.payload)?;
            let edit_shift = read_edit_shift(trak.payload, movie_timescale, track.timescale)?;
            samples.extend(table.expand(kind, track.timescale, edit_shift)?);
        }
        ensure!(
            video.is_some() || audio.is_some(),
            "moov declares no supported track"
        );
        let (samples, duration) = rebase(samples);
        Ok(Self {
            video,
            audio,
            samples,
            duration,
        })
    }

    pub fn samples(&self) -> &[IndexedSample] {
        &self.samples
    }

    /// From the earliest presentation time to the latest sample end: the span
    /// a player shows, which a loop appends the next pass after.
    pub fn duration(&self) -> Timestamp {
        self.duration
    }

    /// The most any sample's presentation time runs ahead of its decode time.
    /// A live encoder with B-frames emits each frame this long after its decode
    /// time, so a sender that does the same sends in decode order at a steady
    /// pace and never sends a sample before its presentation time.
    pub fn reorder_delay(&self) -> Timestamp {
        self.samples
            .iter()
            .map(|sample| sample.pts.saturating_sub(sample.dts))
            .max()
            .unwrap_or(Timestamp::ZERO)
    }

    pub fn annexb_video_sample(&self, avcc: &[u8], is_sync: bool) -> Result<Bytes> {
        let config = self.video.as_ref().context("the file has no video track")?;
        annexb_access_unit(config, avcc, is_sync)
    }
}

struct RawSample {
    kind: SampleKind,
    offset: u64,
    size: u32,
    dts_micros: i64,
    pts_micros: i64,
    end_micros: i64,
    is_sync: bool,
}

struct ChunkRun {
    first_chunk: u32,
    samples_per_chunk: u32,
}

struct SampleTable {
    durations: Vec<u32>,
    composition_offsets: Vec<i32>,
    sizes: Vec<u32>,
    sync_samples: Option<Vec<u32>>,
    chunk_offsets: Vec<u64>,
    chunk_runs: Vec<ChunkRun>,
}

impl SampleTable {
    fn expand(&self, kind: SampleKind, timescale: u32, edit_shift: i64) -> Result<Vec<RawSample>> {
        ensure!(
            self.durations.len() == self.sizes.len(),
            "stts and stsz disagree on the sample count"
        );
        let mut samples = Vec::with_capacity(self.sizes.len());
        let mut decode_time = 0_i64;
        let mut next_sample = 0_usize;
        for (run_index, run) in self.chunk_runs.iter().enumerate() {
            let last_chunk = self
                .chunk_runs
                .get(run_index + 1)
                .map_or(self.chunk_offsets.len() as u32, |next| next.first_chunk - 1);
            for chunk in run.first_chunk..=last_chunk {
                let mut offset = *self
                    .chunk_offsets
                    .get(chunk as usize - 1)
                    .with_context(|| format!("stsc refers to missing chunk {chunk}"))?;
                for _ in 0..run.samples_per_chunk {
                    let Some(&size) = self.sizes.get(next_sample) else {
                        break;
                    };
                    let duration = self.durations[next_sample] as i64;
                    let composition = self
                        .composition_offsets
                        .get(next_sample)
                        .copied()
                        .unwrap_or(0) as i64;
                    let dts = decode_time - edit_shift;
                    let pts = dts + composition;
                    samples.push(RawSample {
                        kind,
                        offset,
                        size,
                        dts_micros: micros(dts, timescale),
                        pts_micros: micros(pts, timescale),
                        end_micros: micros(pts + duration, timescale),
                        is_sync: self.sync_samples.as_ref().is_none_or(|sync| {
                            sync.binary_search(&(next_sample as u32 + 1)).is_ok()
                        }),
                    });
                    offset += size as u64;
                    decode_time += duration;
                    next_sample += 1;
                }
            }
        }
        ensure!(
            next_sample == self.sizes.len(),
            "stsc places {next_sample} of {} samples",
            self.sizes.len()
        );
        Ok(samples)
    }
}

fn micros(ticks: i64, timescale: u32) -> i64 {
    (ticks as i128 * MICROS_PER_SECOND / timescale as i128) as i64
}

/// The earliest decode or presentation time becomes zero, so every sample
/// keeps a non-negative timeline. A version 1 `ctts` can put a presentation
/// time before its decode time (ISO/IEC 14496-12 §8.6.1.3).
fn rebase(mut samples: Vec<RawSample>) -> (Vec<IndexedSample>, Timestamp) {
    let first_presentation = samples
        .iter()
        .map(|sample| sample.pts_micros)
        .min()
        .unwrap_or(0);
    let base = samples
        .iter()
        .map(|sample| sample.dts_micros)
        .min()
        .map_or(first_presentation, |first_decode| {
            first_decode.min(first_presentation)
        });
    let end = samples
        .iter()
        .map(|sample| sample.end_micros)
        .max()
        .unwrap_or(first_presentation);
    samples.sort_by_key(|sample| (sample.dts_micros, sample.kind));
    let indexed = samples
        .into_iter()
        .map(|sample| IndexedSample {
            kind: sample.kind,
            offset: sample.offset,
            size: sample.size,
            dts: Timestamp::from_micros((sample.dts_micros - base) as u64),
            pts: Timestamp::from_micros((sample.pts_micros - base) as u64),
            is_sync: sample.is_sync,
        })
        .collect();
    (
        indexed,
        Timestamp::from_micros((end - first_presentation) as u64),
    )
}

fn read_movie_timescale(moov: &[u8]) -> Result<u32> {
    let mvhd = find(moov, b"mvhd").context("moov has no mvhd")?;
    let timescale = read_timescale(&full_atom(mvhd.payload, "mvhd")?)?;
    ensure!(timescale != 0, "mvhd declares a zero timescale");
    Ok(timescale)
}

/// ISO/IEC 14496-12 §8.6.6: the first edit that maps media places the
/// track's timeline, and an empty edit before it delays the whole track. The
/// shift is in media ticks and is subtracted from every sample time.
fn read_edit_shift(trak: &[u8], movie_timescale: u32, media_timescale: u32) -> Result<i64> {
    let Some(elst) = find_path(trak, &[b"edts", b"elst"]) else {
        return Ok(0);
    };
    let header = full_atom(elst.payload, "elst")?;
    let entry_count = read_u32(header.body, 0)? as usize;
    let mut shift = 0_i64;
    let mut offset = 4;
    for _ in 0..entry_count {
        let (segment_duration, media_time) = if header.is_version_one() {
            let entry = (
                read_u64(header.body, offset)?,
                read_u64(header.body, offset + 8)? as i64,
            );
            offset += 20;
            entry
        } else {
            let entry = (
                read_u32(header.body, offset)? as u64,
                read_u32(header.body, offset + 4)? as i32 as i64,
            );
            offset += 12;
            entry
        };
        if media_time == EMPTY_EDIT_MEDIA_TIME {
            shift -= (segment_duration as i128 * media_timescale as i128 / movie_timescale as i128)
                as i64;
            continue;
        }
        shift += media_time;
        break;
    }
    Ok(shift)
}

fn read_sample_table(stbl: &[u8]) -> Result<SampleTable> {
    let stts = find_full_atom(stbl, "stts")?.context("stbl has no stts")?;
    let mut durations = Vec::new();
    for entry in entries(stts.body, 8)? {
        let count = read_u32(entry, 0)?;
        let delta = read_u32(entry, 4)?;
        durations.extend(std::iter::repeat_n(delta, count as usize));
    }

    let mut composition_offsets = Vec::new();
    if let Some(ctts) = find_full_atom(stbl, "ctts")? {
        for entry in entries(ctts.body, 8)? {
            let count = read_u32(entry, 0)?;
            let composition = read_u32(entry, 4)? as i32;
            composition_offsets.extend(std::iter::repeat_n(composition, count as usize));
        }
    }

    let stsz = find_full_atom(stbl, "stsz")?.context("stbl has no stsz")?;
    let uniform_size = read_u32(stsz.body, 0)?;
    let sample_count = read_u32(stsz.body, 4)? as usize;
    let sizes = if uniform_size != 0 {
        vec![uniform_size; sample_count]
    } else {
        let body = stsz.body.get(4..).context("stsz is truncated")?;
        entries(body, 4)?
            .map(|entry| read_u32(entry, 0))
            .collect::<Result<Vec<_>>>()?
    };
    ensure!(
        sizes.len() == sample_count,
        "stsz lists fewer sizes than samples"
    );

    let sync_samples = match find(stbl, b"stss") {
        Some(stss) => {
            let mut sync = entries(full_atom(stss.payload, "stss")?.body, 4)?
                .map(|entry| read_u32(entry, 0))
                .collect::<Result<Vec<_>>>()?;
            // A malformed stss may be unordered; the per-sample `binary_search` needs
            // ascending order.
            sync.sort_unstable();
            Some(sync)
        }
        None => None,
    };

    let stsc = find_full_atom(stbl, "stsc")?.context("stbl has no stsc")?;
    let chunk_runs = entries(stsc.body, 12)?
        .map(|entry| {
            Ok(ChunkRun {
                first_chunk: read_u32(entry, 0)?,
                samples_per_chunk: read_u32(entry, 4)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        chunk_runs.first().is_some_and(|run| run.first_chunk == 1),
        "stsc does not start at the first chunk"
    );

    let chunk_offsets = if let Some(stco) = find(stbl, b"stco") {
        entries(full_atom(stco.payload, "stco")?.body, 4)?
            .map(|entry| read_u32(entry, 0).map(u64::from))
            .collect::<Result<Vec<_>>>()?
    } else {
        let co64 = find(stbl, b"co64").context("stbl has neither stco nor co64")?;
        entries(full_atom(co64.payload, "co64")?.body, 8)?
            .map(|entry| read_u64(entry, 0))
            .collect::<Result<Vec<_>>>()?
    };

    Ok(SampleTable {
        durations,
        composition_offsets,
        sizes,
        sync_samples,
        chunk_offsets,
        chunk_runs,
    })
}

/// A counted table: an entry count followed by fixed-size entries.
fn entries(body: &[u8], entry_size: usize) -> Result<impl Iterator<Item = &[u8]>> {
    let count = read_u32(body, 0)? as usize;
    let table = body
        .get(4..4 + count * entry_size)
        .context("sample table is shorter than its entry count")?;
    Ok(table.chunks_exact(entry_size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mp4::Demuxer,
        sample::MediaEvent,
        test_support::{FIXTURE_MP3_MP4, FIXTURE_PROGRESSIVE_MP4},
    };

    fn moov(file: &[u8]) -> &[u8] {
        let mut offset = 0;
        while let Some((size, kind)) = peek(&file[offset..]) {
            if kind == b"moov" {
                return &file[offset..offset + size];
            }
            offset += size;
        }
        panic!("fixture has no moov atom");
    }

    fn of_kind(index: &Mp4Index, kind: SampleKind) -> Vec<&IndexedSample> {
        index
            .samples()
            .iter()
            .filter(|sample| sample.kind == kind)
            .collect()
    }

    fn bytes_of<'a>(file: &'a [u8], sample: &IndexedSample) -> &'a [u8] {
        &file[sample.offset as usize..sample.offset as usize + sample.size as usize]
    }

    #[test]
    fn indexes_the_tracks_of_a_progressive_file_in_decode_order() {
        // Arrange / Act
        let index = Mp4Index::parse(moov(FIXTURE_PROGRESSIVE_MP4)).unwrap();

        // Assert
        let video = of_kind(&index, SampleKind::Video);
        let audio = of_kind(&index, SampleKind::Audio);
        assert_eq!((video.len(), audio.len()), (10, 31));
        assert_eq!(
            video.iter().filter(|sample| sample.is_sync).count(),
            2,
            "one keyframe per 8-frame GOP"
        );
        assert!(video[0].is_sync && !video[1].is_sync);
        assert!(
            index
                .samples()
                .windows(2)
                .all(|pair| pair[0].dts <= pair[1].dts)
        );
        assert!(index.samples().iter().all(|sample| {
            (sample.offset + sample.size as u64) as usize <= FIXTURE_PROGRESSIVE_MP4.len()
        }));
        assert_eq!(video[0].size, 2185);
        assert!(index.duration().micros() > 600_000 && index.duration().micros() < 700_000);
    }

    #[test]
    fn applies_composition_offsets_and_edit_lists_to_b_frame_video() {
        // Arrange
        let index = Mp4Index::parse(moov(FIXTURE_PROGRESSIVE_MP4)).unwrap();
        let video = of_kind(&index, SampleKind::Video);
        let audio = of_kind(&index, SampleKind::Audio);

        // Assert: the second frame in decode order is shown after the third
        assert!(video[1].pts > video[2].pts);
        assert!(video.windows(2).all(|pair| pair[0].dts < pair[1].dts));
        // Assert: the edit lists line the tracks up, so the picture follows the
        // audio by its 1024 priming samples and the timeline starts at zero
        assert_eq!(index.samples()[0].dts.micros(), 0);
        assert_eq!(video[0].pts.micros() - audio[0].pts.micros(), 21_333);
    }

    #[test]
    fn reorder_delay_is_the_largest_lead_of_presentation_over_decode_time() {
        // Arrange
        let b_frames = Mp4Index::parse(moov(FIXTURE_PROGRESSIVE_MP4)).unwrap();
        let no_b_frames = Mp4Index::parse(moov(FIXTURE_MP3_MP4)).unwrap();

        // Act
        let delays = (b_frames.reorder_delay(), no_b_frames.reorder_delay());

        // Assert: a P-frame shown four 15 fps frames after it is decoded
        assert_eq!(delays.0.micros(), 266_667);
        assert_eq!(delays.1, Timestamp::ZERO);
        assert!(
            b_frames
                .samples()
                .iter()
                .all(|sample| sample.dts.saturating_add(delays.0) >= sample.pts)
        );
    }

    #[test]
    fn converts_video_samples_to_annex_b_with_parameter_sets_on_keyframes() {
        // Arrange
        let index = Mp4Index::parse(moov(FIXTURE_PROGRESSIVE_MP4)).unwrap();
        let video = of_kind(&index, SampleKind::Video);
        let config = index.video.as_ref().unwrap();

        // Act
        let keyframe = index
            .annexb_video_sample(bytes_of(FIXTURE_PROGRESSIVE_MP4, video[0]), true)
            .unwrap();
        let delta = index
            .annexb_video_sample(bytes_of(FIXTURE_PROGRESSIVE_MP4, video[1]), false)
            .unwrap();

        // Assert
        assert!(keyframe.starts_with(&config.parameter_sets_annexb()));
        assert!(delta.starts_with(&[0, 0, 0, 1]));
        assert!(!delta.starts_with(&config.parameter_sets_annexb()));
        assert_eq!(config.codec_string(), "avc1.4D400A");
    }

    #[test]
    fn indexes_mp3_audio_as_raw_frames() {
        // Arrange / Act
        let index = Mp4Index::parse(moov(FIXTURE_MP3_MP4)).unwrap();

        // Assert
        assert_eq!(
            index.audio,
            Some(AudioConfig::Mp3 {
                sample_rate: 48_000,
                channels: 1
            })
        );
        let audio = of_kind(&index, SampleKind::Audio);
        assert_eq!(audio.len(), 28);
        assert!(audio.iter().all(|sample| sample.is_sync));
        for (number, sample) in audio.iter().enumerate() {
            let frame = bytes_of(FIXTURE_MP3_MP4, sample);
            assert_eq!(
                (frame[0], frame[1] & 0xE0),
                (0xFF, 0xE0),
                "MPEG audio sync word of frame {number} at offset {}",
                sample.offset
            );
        }
    }

    #[test]
    fn fragment_demuxer_describes_mp3_audio() {
        // Arrange
        let mut demuxer = Demuxer::new();

        // Act
        let events = demuxer.push(moov(FIXTURE_MP3_MP4)).unwrap();

        // Assert
        assert!(events.contains(&MediaEvent::AudioConfig(AudioConfig::Mp3 {
            sample_rate: 48_000,
            channels: 1
        })));
    }

    mod properties {
        use proptest::{collection::vec, prelude::*};

        use super::super::{ChunkRun, SampleKind, SampleTable, micros, rebase};

        /// A track as its `stbl` and edit list describe it, with each sample's
        /// `(duration, composition offset, size)` and the number of samples in
        /// each chunk.
        #[derive(Debug)]
        struct Track {
            timescale: u32,
            edit_shift: i64,
            samples: Vec<(u32, i32, u32)>,
            chunk_lengths: Vec<usize>,
        }

        impl Track {
            /// Consecutive chunks of the same length share one `stsc` run.
            fn table(&self) -> SampleTable {
                let mut chunk_runs: Vec<ChunkRun> = Vec::new();
                let mut chunk_offsets = Vec::new();
                let mut offset = 0u64;
                let mut sample = 0;
                for (index, &length) in self.chunk_lengths.iter().enumerate() {
                    if chunk_runs
                        .last()
                        .is_none_or(|run| run.samples_per_chunk as usize != length)
                    {
                        chunk_runs.push(ChunkRun {
                            first_chunk: index as u32 + 1,
                            samples_per_chunk: length as u32,
                        });
                    }
                    chunk_offsets.push(offset);
                    let sizes = &self.samples[sample..sample + length];
                    offset += sizes
                        .iter()
                        .map(|&(_, _, size)| u64::from(size))
                        .sum::<u64>()
                        + 100;
                    sample += length;
                }
                SampleTable {
                    durations: self
                        .samples
                        .iter()
                        .map(|&(duration, _, _)| duration)
                        .collect(),
                    composition_offsets: self
                        .samples
                        .iter()
                        .map(|&(_, offset, _)| offset)
                        .collect(),
                    sizes: self.samples.iter().map(|&(_, _, size)| size).collect(),
                    sync_samples: None,
                    chunk_offsets,
                    chunk_runs,
                }
            }

            /// Each sample's decode and presentation time in microseconds,
            /// before rebasing, in sample order.
            fn times(&self) -> Vec<(i64, i64)> {
                let mut decode_time = -self.edit_shift;
                self.samples
                    .iter()
                    .map(|&(duration, composition, _)| {
                        let times = (
                            micros(decode_time, self.timescale),
                            micros(decode_time + i64::from(composition), self.timescale),
                        );
                        decode_time += i64::from(duration);
                        times
                    })
                    .collect()
            }
        }

        /// Composition offsets of either sign: a version 1 `ctts` may put a
        /// presentation time before its decode time.
        fn track() -> impl Strategy<Value = Track> {
            let sample = (0..3_000u32, -6_000..6_000i32, 1..2_000u32);
            (
                prop_oneof![Just(1_000u32), Just(48_000), Just(90_000)],
                prop_oneof![Just(0i64), -6_000..6_000i64],
                vec(vec(sample, 1..4), 1..6),
            )
                .prop_map(|(timescale, edit_shift, chunks)| Track {
                    timescale,
                    edit_shift,
                    chunk_lengths: chunks.iter().map(Vec::len).collect(),
                    samples: chunks.into_iter().flatten().collect(),
                })
        }

        proptest! {
            #[test]
            fn the_index_starts_at_zero_and_keeps_each_samples_times(track in track()) {
                // Arrange
                let table = track.table();

                // Act
                let raw = table.expand(SampleKind::Video, track.timescale, track.edit_shift).unwrap();
                let (indexed, _) = rebase(raw);

                // Assert: samples keep their order, place and the gaps between
                // all their times; the earliest time of any sample is zero
                let times = track.times();
                let base = times.iter().flat_map(|&(dts, pts)| [dts, pts]).min().unwrap();
                prop_assert_eq!(indexed.len(), times.len());
                let mut offset = None;
                for (sample, &(dts, pts)) in indexed.iter().zip(&times) {
                    prop_assert_eq!(sample.dts.micros() as i64, dts - base);
                    prop_assert_eq!(sample.pts.micros() as i64, pts - base);
                    prop_assert!(offset.is_none_or(|offset| sample.offset >= offset));
                    offset = Some(sample.offset + u64::from(sample.size));
                }
            }
        }
    }
}
