use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use bytes::{Bytes, BytesMut};

use crate::{
    aac::AudioSpecificConfig,
    h264::{AvcDecoderConfigurationRecord, avcc::avcc_to_annexb},
    mp4::atom::{
        Atom, HEADER_LENGTH as ATOM_HEADER_LENGTH, atoms, find, full_atom, peek, read_u32, read_u64,
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
const VIDEO_SAMPLE_ENTRIES: [&[u8]; 2] = [b"avc1", b"avc3"];
const VISUAL_SAMPLE_ENTRY_LENGTH: usize = 78;
const AUDIO_SAMPLE_ENTRY_LENGTH: usize = 28;
const AUDIO_SAMPLE_ENTRY: &[u8] = b"mp4a";
const DEFAULT_TIMESCALE: u32 = 90_000;
const AAC_LC_OBJECT_TYPE: u8 = 2;

#[derive(Default)]
pub struct Demuxer {
    buffer: BytesMut,
    tracks: BTreeMap<u32, Track>,
    pending: Option<Fragment>,
}

struct Track {
    media: Media,
    timescale: u32,
}

enum Media {
    Video(AvcDecoderConfigurationRecord),
    Audio(AudioSpecificConfig),
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

    fn read_movie(&mut self, payload: &[u8]) -> Result<Vec<MediaEvent>> {
        for trak in atoms(payload).filter(|atom| atom.kind == b"trak") {
            let (track_id, track) = read_track(&trak)?;
            self.tracks.insert(track_id, track);
        }
        ensure!(!self.tracks.is_empty(), "moov declares no supported track");

        let mut events = vec![MediaEvent::Streams(StreamSet {
            has_video: self.has(|media| matches!(media, Media::Video(_))),
            has_audio: self.has(|media| matches!(media, Media::Audio(_))),
        })];
        for track in self.tracks.values() {
            events.push(match &track.media {
                Media::Video(config) => MediaEvent::VideoConfig(config.clone()),
                Media::Audio(config) => MediaEvent::AudioConfig(config.clone()),
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
            Media::Video(config) => {
                let annexb = avcc_to_annexb(&data, config.nal_length_size as usize)?;
                MediaEvent::Video(VideoSample {
                    data: if self.is_sync {
                        config.with_parameter_sets(annexb)
                    } else {
                        annexb
                    },
                    is_keyframe: self.is_sync,
                    pts,
                    dts,
                })
            }
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

fn read_track(trak: &Atom) -> Result<(u32, Track)> {
    let tkhd = find(trak.payload, b"tkhd").context("trak has no tkhd")?;
    let header = full_atom(tkhd.payload, "tkhd")?;
    let track_id_offset = if header.is_version_one() { 16 } else { 8 };
    let track_id = read_u32(header.body, track_id_offset)?;

    let mdia = find(trak.payload, b"mdia").context("trak has no mdia")?;
    let mdhd = find(mdia.payload, b"mdhd").context("mdia has no mdhd")?;
    let header = full_atom(mdhd.payload, "mdhd")?;
    let timescale = if header.is_version_one() {
        read_u32(header.body, 16)?
    } else {
        read_u32(header.body, 8)?
    };

    let minf = find(mdia.payload, b"minf").context("mdia has no minf")?;
    let stbl = find(minf.payload, b"stbl").context("minf has no stbl")?;
    let stsd = find(stbl.payload, b"stsd").context("stbl has no stsd")?;
    let media = read_sample_entry(full_atom(stsd.payload, "stsd")?.body)?;

    Ok((
        track_id,
        Track {
            media,
            timescale: if timescale == 0 {
                DEFAULT_TIMESCALE
            } else {
                timescale
            },
        },
    ))
}

fn read_sample_entry(body: &[u8]) -> Result<Media> {
    let entries = body.get(4..).context("stsd has no entries")?;
    for entry in atoms(entries) {
        if VIDEO_SAMPLE_ENTRIES.contains(&entry.kind) {
            let children = entry
                .payload
                .get(VISUAL_SAMPLE_ENTRY_LENGTH..)
                .context("visual sample entry is truncated")?;
            let avcc = find(children, b"avcC").context("video sample entry has no avcC")?;
            return Ok(Media::Video(AvcDecoderConfigurationRecord::parse(
                avcc.payload,
            )?));
        }
        if entry.kind == AUDIO_SAMPLE_ENTRY {
            return Ok(Media::Audio(read_audio_config(entry.payload)?));
        }
    }
    anyhow::bail!("stsd carries no H.264 or AAC sample entry")
}

/// The esds decoder specific info is optional, so a sample entry without one
/// falls back to the channel count and sample rate the entry itself declares.
fn read_audio_config(payload: &[u8]) -> Result<AudioSpecificConfig> {
    let children = payload
        .get(AUDIO_SAMPLE_ENTRY_LENGTH..)
        .context("audio sample entry is truncated")?;
    let specific_info = find(children, b"esds")
        .and_then(|esds| full_atom(esds.payload, "esds").ok())
        .and_then(|esds| decoder_specific_info(esds.body))
        .map(AudioSpecificConfig::parse);
    if let Some(config) = specific_info {
        return config;
    }

    let channels = u16::from_be_bytes(
        payload
            .get(16..18)
            .context("audio sample entry has no channel count")?
            .try_into()?,
    );
    let sample_rate = read_u32(payload, 24)? >> 16;
    Ok(AudioSpecificConfig::new(
        AAC_LC_OBJECT_TYPE,
        sample_rate,
        channels as u8,
    ))
}

/// ISO/IEC 14496-1 descriptors: an ES descriptor (0x03) holds a decoder config
/// descriptor (0x04) that may hold decoder specific info (0x05).
fn decoder_specific_info(mut body: &[u8]) -> Option<&[u8]> {
    loop {
        let (tag, payload, rest) = read_descriptor(body).ok()?;
        body = match tag {
            0x03 => payload.get(3..)?,
            0x04 => payload.get(13..)?,
            0x05 => return Some(payload),
            _ => rest,
        };
        if body.is_empty() {
            return None;
        }
    }
}

fn read_descriptor(data: &[u8]) -> Result<(u8, &[u8], &[u8])> {
    let tag = *data.first().context("descriptor is empty")?;
    let mut length = 0_usize;
    let mut offset = 1;
    loop {
        ensure!(offset <= 4, "descriptor length exceeds four bytes");
        let byte = *data.get(offset).context("descriptor length is truncated")?;
        length = (length << 7) | (byte & 0x7F) as usize;
        offset += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }
    let payload = data
        .get(offset..offset + length)
        .context("descriptor payload is truncated")?;
    Ok((tag, payload, &data[offset + length..]))
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
