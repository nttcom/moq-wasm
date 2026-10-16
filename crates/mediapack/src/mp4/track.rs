use anyhow::{Context, Result, ensure};
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig,
    av1,
    h264::{AvcDecoderConfigurationRecord, avcc::avcc_to_annexb},
    mp4::atom::{Atom, FullAtom, find, full_atom, read_u32},
    sample::{AudioConfig, VideoConfig},
    vp8::Vp8Config,
    vp9::Vp9Config,
};

const AVC_SAMPLE_ENTRIES: [&[u8]; 2] = [b"avc1", b"avc3"];
const VP8_SAMPLE_ENTRY: &[u8] = b"vp08";
const VP9_SAMPLE_ENTRY: &[u8] = b"vp09";
const AV1_SAMPLE_ENTRY: &[u8] = b"av01";
// VP codec ISOBMFF binding §2.2 chromaSubsampling values.
const VP_SUBSAMPLING_422: u8 = 2;
const VP_SUBSAMPLING_444: u8 = 3;
/// AV1-ISOBMFF binding §2.3: the marker/version byte and the three bytes of
/// fields before the configOBUs.
const AV1C_HEADER_LENGTH: usize = 4;
const VISUAL_SAMPLE_ENTRY_LENGTH: usize = 78;
const AUDIO_SAMPLE_ENTRY_LENGTH: usize = 28;
const MPEG4_AUDIO_SAMPLE_ENTRY: &[u8] = b"mp4a";
const MP3_SAMPLE_ENTRY: &[u8] = b".mp3";
const OPUS_SAMPLE_ENTRY: &[u8] = b"Opus";
const ALAW_SAMPLE_ENTRY: &[u8] = b"alaw";
const DEFAULT_TIMESCALE: u32 = 90_000;
const AAC_LC_OBJECT_TYPE: u8 = 2;
/// ISO/IEC 14496-1 Table 5: objectTypeIndication of MPEG-2 and MPEG-1 audio.
pub(super) const MPEG2_AUDIO_OBJECT_TYPE_INDICATION: u8 = 0x69;
pub(super) const MPEG1_AUDIO_OBJECT_TYPE_INDICATION: u8 = 0x6B;

pub(super) enum Media {
    Video(VideoConfig),
    Audio(AudioConfig),
}

pub(super) struct Track {
    pub media: Media,
    pub timescale: u32,
}

/// ISO/IEC 14496-12 §8.2.2 and §8.4.2: mvhd and mdhd both put the timescale
/// after a creation and a modification time of 32 bits, or 64 bits in version 1.
pub(super) fn read_timescale(header: &FullAtom) -> Result<u32> {
    read_u32(header.body, if header.is_version_one() { 16 } else { 8 })
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

pub(super) fn read_track(trak: &Atom) -> Result<Option<(u32, Track)>> {
    let tkhd = find(trak.payload, b"tkhd").context("trak has no tkhd")?;
    let header = full_atom(tkhd.payload, "tkhd")?;
    let track_id_offset = if header.is_version_one() { 16 } else { 8 };
    let track_id = read_u32(header.body, track_id_offset)?;

    let mdhd = find_path(trak.payload, &[b"mdia", b"mdhd"]).context("trak has no mdhd")?;
    let timescale = read_timescale(&full_atom(mdhd.payload, "mdhd")?)?;

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
        if let Some(config) = read_video_entry(entry.kind, entry.payload)? {
            return Ok(Some(Media::Video(config)));
        }
        if entry.kind == MPEG4_AUDIO_SAMPLE_ENTRY {
            return Ok(Some(Media::Audio(read_mpeg4_audio(entry.payload)?)));
        }
        if entry.kind == MP3_SAMPLE_ENTRY {
            return Ok(Some(Media::Audio(read_mp3(entry.payload)?)));
        }
        if entry.kind == OPUS_SAMPLE_ENTRY {
            return Ok(Some(Media::Audio(read_opus(entry.payload)?)));
        }
        if entry.kind == ALAW_SAMPLE_ENTRY {
            let (sample_rate, channels) = read_audio_entry_format(entry.payload)?;
            return Ok(Some(Media::Audio(AudioConfig::Pcma {
                sample_rate,
                channels,
            })));
        }
    }
    Ok(None)
}

fn read_video_entry(kind: &[u8], payload: &[u8]) -> Result<Option<VideoConfig>> {
    let is_video = AVC_SAMPLE_ENTRIES.contains(&kind)
        || [VP8_SAMPLE_ENTRY, VP9_SAMPLE_ENTRY, AV1_SAMPLE_ENTRY].contains(&kind);
    if !is_video {
        return Ok(None);
    }
    let children = payload
        .get(VISUAL_SAMPLE_ENTRY_LENGTH..)
        .context("visual sample entry is truncated")?;
    // ISO/IEC 14496-12 §12.1.3: the 16-bit width and height follow 24 bytes
    // of SampleEntry and pre-defined fields.
    let size = read_u32(payload, 24)?;
    let (width, height) = (size >> 16, size & 0xFFFF);
    Ok(Some(match kind {
        VP8_SAMPLE_ENTRY => VideoConfig::Vp8(Vp8Config { width, height }),
        VP9_SAMPLE_ENTRY => VideoConfig::Vp9(read_vp9(children, width, height)?),
        AV1_SAMPLE_ENTRY => VideoConfig::Av1(read_av1(children)?),
        _ => {
            let avcc = find(children, b"avcC").context("video sample entry has no avcC")?;
            VideoConfig::Avc(AvcDecoderConfigurationRecord::parse(avcc.payload)?)
        }
    }))
}

/// VPCodecConfigurationBox (VP codec ISOBMFF binding §2.2).
fn read_vp9(children: &[u8], width: u32, height: u32) -> Result<Vp9Config> {
    let vpcc = find(children, b"vpcC").context("VP9 sample entry has no vpcC")?;
    let body = full_atom(vpcc.payload, "vpcC")?.body;
    let profile = *body.first().context("vpcC has no profile")?;
    let packed = *body.get(2).context("vpcC has no bit depth")?;
    let (subsampling_x, subsampling_y) = match packed >> 1 & 0x07 {
        VP_SUBSAMPLING_422 => (true, false),
        VP_SUBSAMPLING_444 => (false, false),
        _ => (true, true),
    };
    Ok(Vp9Config {
        profile,
        bit_depth: packed >> 4,
        subsampling_x,
        subsampling_y,
        full_range: packed & 1 == 1,
        width,
        height,
    })
}

/// The configOBUs of an AV1CodecConfigurationBox (AV1-ISOBMFF binding §2.3)
/// hold the sequence header OBU, which states every field of the config.
fn read_av1(children: &[u8]) -> Result<av1::Av1Config> {
    let av1c = find(children, b"av1C").context("AV1 sample entry has no av1C")?;
    let config_obus = av1c
        .payload
        .get(AV1C_HEADER_LENGTH..)
        .context("av1C is truncated")?;
    av1::read_sequence_header(&Bytes::copy_from_slice(config_obus))?
        .context("av1C has no sequence header OBU")
}

/// The OpusSpecificBox states the encoder's input rate and channel count; the
/// sample entry itself always says 48 kHz.
fn read_opus(payload: &[u8]) -> Result<AudioConfig> {
    let children = payload
        .get(AUDIO_SAMPLE_ENTRY_LENGTH..)
        .context("audio sample entry is truncated")?;
    let dops = find(children, b"dOps").context("Opus sample entry has no dOps")?;
    let channels = *dops.payload.get(1).context("dOps has no channel count")?;
    Ok(AudioConfig::Opus {
        sample_rate: read_u32(dops.payload, 4)?,
        channels,
    })
}

/// The esds decoder specific info is optional, so an AAC sample entry without
/// one falls back to the channel count and sample rate the entry itself
/// declares. MP3 in an `mp4a` entry is told apart by the objectTypeIndication
/// of its decoder config descriptor.
fn read_mpeg4_audio(payload: &[u8]) -> Result<AudioConfig> {
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
        }) => Ok(AudioConfig::Aac(AudioSpecificConfig::parse(info)?)),
        _ => {
            let (sample_rate, channels) = read_audio_entry_format(payload)?;
            Ok(AudioConfig::Aac(AudioSpecificConfig::new(
                AAC_LC_OBJECT_TYPE,
                sample_rate,
                channels,
            )))
        }
    }
}

fn read_mp3(payload: &[u8]) -> Result<AudioConfig> {
    let (sample_rate, channels) = read_audio_entry_format(payload)?;
    Ok(AudioConfig::Mp3 {
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
