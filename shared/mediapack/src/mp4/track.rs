use anyhow::{Context, Result, ensure};
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig,
    h264::{AvcDecoderConfigurationRecord, avcc::avcc_to_annexb},
    mp4::atom::{Atom, find, full_atom, read_u32},
};

const VIDEO_SAMPLE_ENTRIES: [&[u8]; 2] = [b"avc1", b"avc3"];
const VISUAL_SAMPLE_ENTRY_LENGTH: usize = 78;
const AUDIO_SAMPLE_ENTRY_LENGTH: usize = 28;
const MPEG4_AUDIO_SAMPLE_ENTRY: &[u8] = b"mp4a";
const MP3_SAMPLE_ENTRY: &[u8] = b".mp3";
const DEFAULT_TIMESCALE: u32 = 90_000;
const AAC_LC_OBJECT_TYPE: u8 = 2;
/// ISO/IEC 14496-1 Table 5: objectTypeIndication of MPEG-2 and MPEG-1 audio.
const MPEG2_AUDIO_OBJECT_TYPE_INDICATION: u8 = 0x69;
const MPEG1_AUDIO_OBJECT_TYPE_INDICATION: u8 = 0x6B;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioCodec {
    Aac(AudioSpecificConfig),
    Mp3 { sample_rate: u32, channels: u8 },
}

pub(super) enum Media {
    Video(AvcDecoderConfigurationRecord),
    Audio(AudioCodec),
}

pub(super) struct Track {
    pub media: Media,
    pub timescale: u32,
}

pub(super) fn find_path<'a>(payload: &'a [u8], path: &[&[u8]]) -> Option<Atom<'a>> {
    let (first, rest) = path.split_first()?;
    let atom = find(payload, first)?;
    if rest.is_empty() {
        Some(atom)
    } else {
        find_path(atom.payload, rest)
    }
}

/// Tracks whose sample entry is neither H.264 nor MPEG audio are reported as
/// `None` so that callers can leave them out.
pub(super) fn read_track(trak: &Atom) -> Result<Option<(u32, Track)>> {
    let tkhd = find(trak.payload, b"tkhd").context("trak has no tkhd")?;
    let header = full_atom(tkhd.payload, "tkhd")?;
    let track_id_offset = if header.is_version_one() { 16 } else { 8 };
    let track_id = read_u32(header.body, track_id_offset)?;

    let mdhd = find_path(trak.payload, &[b"mdia", b"mdhd"]).context("trak has no mdhd")?;
    let header = full_atom(mdhd.payload, "mdhd")?;
    let timescale = if header.is_version_one() {
        read_u32(header.body, 16)?
    } else {
        read_u32(header.body, 8)?
    };

    let stsd = find_path(trak.payload, &[b"mdia", b"minf", b"stbl", b"stsd"])
        .context("trak has no stsd")?;
    let Some(media) = read_sample_entry(full_atom(stsd.payload, "stsd")?.body)? else {
        return Ok(None);
    };

    Ok(Some((
        track_id,
        Track {
            media,
            timescale: if timescale == 0 {
                DEFAULT_TIMESCALE
            } else {
                timescale
            },
        },
    )))
}

/// Keyframes carry the SPS / PPS of `config` in-band, as the LOC payload
/// format and the other demuxers of this crate deliver them.
pub(super) fn annexb_access_unit(
    config: &AvcDecoderConfigurationRecord,
    avcc: &[u8],
    is_sync: bool,
) -> Result<Bytes> {
    let annexb = avcc_to_annexb(avcc, config.nal_length_size as usize)?;
    Ok(if is_sync {
        config.with_parameter_sets(annexb)
    } else {
        annexb
    })
}

fn read_sample_entry(body: &[u8]) -> Result<Option<Media>> {
    let entries = body.get(4..).context("stsd has no entries")?;
    for entry in crate::mp4::atom::atoms(entries) {
        if VIDEO_SAMPLE_ENTRIES.contains(&entry.kind) {
            let children = entry
                .payload
                .get(VISUAL_SAMPLE_ENTRY_LENGTH..)
                .context("visual sample entry is truncated")?;
            let avcc = find(children, b"avcC").context("video sample entry has no avcC")?;
            return Ok(Some(Media::Video(AvcDecoderConfigurationRecord::parse(
                avcc.payload,
            )?)));
        }
        if entry.kind == MPEG4_AUDIO_SAMPLE_ENTRY {
            return Ok(Some(Media::Audio(read_mpeg4_audio(entry.payload)?)));
        }
        if entry.kind == MP3_SAMPLE_ENTRY {
            return Ok(Some(Media::Audio(read_mp3(entry.payload)?)));
        }
    }
    Ok(None)
}

/// The esds decoder specific info is optional, so an AAC sample entry without
/// one falls back to the channel count and sample rate the entry itself
/// declares. MP3 in an `mp4a` entry is told apart by the objectTypeIndication
/// of its decoder config descriptor.
fn read_mpeg4_audio(payload: &[u8]) -> Result<AudioCodec> {
    let children = payload
        .get(AUDIO_SAMPLE_ENTRY_LENGTH..)
        .context("audio sample entry is truncated")?;
    let descriptor = find(children, b"esds")
        .and_then(|esds| full_atom(esds.payload, "esds").ok())
        .and_then(|esds| read_es_descriptor(esds.body));
    match descriptor {
        Some(EsDescriptor {
            object_type_indication:
                MPEG1_AUDIO_OBJECT_TYPE_INDICATION | MPEG2_AUDIO_OBJECT_TYPE_INDICATION,
            ..
        }) => read_mp3(payload),
        Some(EsDescriptor {
            specific_info: Some(info),
            ..
        }) => Ok(AudioCodec::Aac(AudioSpecificConfig::parse(info)?)),
        _ => {
            let (sample_rate, channels) = read_audio_entry_format(payload)?;
            Ok(AudioCodec::Aac(AudioSpecificConfig::new(
                AAC_LC_OBJECT_TYPE,
                sample_rate,
                channels,
            )))
        }
    }
}

fn read_mp3(payload: &[u8]) -> Result<AudioCodec> {
    let (sample_rate, channels) = read_audio_entry_format(payload)?;
    Ok(AudioCodec::Mp3 {
        sample_rate,
        channels,
    })
}

fn read_audio_entry_format(payload: &[u8]) -> Result<(u32, u8)> {
    let channels = u16::from_be_bytes(
        payload
            .get(16..18)
            .context("audio sample entry has no channel count")?
            .try_into()?,
    );
    let sample_rate = read_u32(payload, 24)? >> 16;
    Ok((sample_rate, channels as u8))
}

struct EsDescriptor<'a> {
    object_type_indication: u8,
    specific_info: Option<&'a [u8]>,
}

/// ISO/IEC 14496-1 descriptors: an ES descriptor (0x03) holds a decoder config
/// descriptor (0x04), whose first byte is the objectTypeIndication and which
/// may hold decoder specific info (0x05).
fn read_es_descriptor(mut body: &[u8]) -> Option<EsDescriptor<'_>> {
    let mut object_type_indication = None;
    loop {
        let (tag, payload, rest) = read_descriptor(body).ok()?;
        body = match tag {
            0x03 => payload.get(3..)?,
            0x04 => {
                object_type_indication = payload.first().copied();
                payload.get(13..)?
            }
            0x05 => {
                return Some(EsDescriptor {
                    object_type_indication: object_type_indication?,
                    specific_info: Some(payload),
                });
            }
            _ => rest,
        };
        if body.is_empty() {
            return Some(EsDescriptor {
                object_type_indication: object_type_indication?,
                specific_info: None,
            });
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
