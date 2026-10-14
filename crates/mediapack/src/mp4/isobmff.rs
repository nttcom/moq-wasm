use anyhow::{Context, Result, bail, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{
    aac::asc::SAMPLES_PER_FRAME,
    h264::{AvcDecoderConfigurationRecord, annexb::annexb_to_avcc_without_parameter_sets},
    mp4::track::{MPEG1_AUDIO_OBJECT_TYPE_INDICATION, MPEG2_AUDIO_OBJECT_TYPE_INDICATION},
    sample::{AudioConfig, AudioSample, VideoSample},
};

pub(super) const VIDEO_TIMESCALE: u32 = 90_000;
/// ISO/IEC 14496-1 Table 5: objectTypeIndication of MPEG-4 audio.
const AAC_OBJECT_TYPE_INDICATION: u8 = 0x40;
/// Opus in ISO BMFF (the Opus-in-ISOBMFF binding §4.3.2) always counts
/// samples at 48 kHz, whatever rate the encoder was fed.
const OPUS_TIMESCALE: u32 = 48_000;
const MOVIE_TIMESCALE: u32 = 1_000;
pub(super) const SYNC_SAMPLE_FLAGS: u32 = 0x0200_0000;
pub(super) const NON_SYNC_SAMPLE_FLAGS: u32 = 0x0101_0000;
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x0002_0000;
const TRUN_FLAGS: u32 = 0x0001 | 0x0100 | 0x0200 | 0x0400 | 0x0800;
const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

pub(super) struct TrackInit {
    pub(super) track_id: u32,
    pub(super) trak: Bytes,
}

pub(super) fn init_segment(tracks: &[TrackInit]) -> Bytes {
    let next_track_id = tracks.iter().map(|track| track.track_id).max().unwrap_or(0) + 1;
    let mut moov = BytesMut::from(mvhd(next_track_id).as_ref());
    let mut mvex = BytesMut::new();
    for track in tracks {
        moov.put_slice(&track.trak);
        mvex.put_slice(&trex(track.track_id));
    }
    moov.put_slice(&plain_box(b"mvex", &mvex));
    let mut out = BytesMut::from(ftyp().as_ref());
    out.put_slice(&plain_box(b"moov", &moov));
    out.freeze()
}

pub(super) fn video_fragment(
    sequence_number: u32,
    track_id: u32,
    sample: &VideoSample,
    duration: u32,
    nal_length_size: usize,
) -> Bytes {
    let data = annexb_to_avcc_without_parameter_sets(&sample.data, nal_length_size);
    let composition_offset =
        sample.pts.ticks(VIDEO_TIMESCALE) as i64 - sample.dts.ticks(VIDEO_TIMESCALE) as i64;
    let flags = if sample.is_keyframe {
        SYNC_SAMPLE_FLAGS
    } else {
        NON_SYNC_SAMPLE_FLAGS
    };
    moof_mdat(
        sequence_number,
        track_id,
        sample.dts.ticks(VIDEO_TIMESCALE),
        duration,
        flags,
        composition_offset as i32,
        &data,
    )
}

/// Fails for an MP3 frame without a valid header, an empty or truncated Opus
/// packet, or A-law with no channels.
pub(super) fn audio_fragment(
    sequence_number: u32,
    track_id: u32,
    sample: &AudioSample,
    config: &AudioConfig,
) -> Result<Bytes> {
    Ok(moof_mdat(
        sequence_number,
        track_id,
        sample.pts.ticks(audio_timescale(config)),
        audio_sample_duration(config, &sample.data)?,
        SYNC_SAMPLE_FLAGS,
        0,
        &sample.data,
    ))
}

fn audio_timescale(config: &AudioConfig) -> u32 {
    match config {
        AudioConfig::Opus { .. } => OPUS_TIMESCALE,
        other => other.sample_rate(),
    }
}

/// The duration of one encoded frame in ticks of `audio_timescale`.
fn audio_sample_duration(config: &AudioConfig, data: &[u8]) -> Result<u32> {
    match config {
        AudioConfig::Aac(_) => Ok(SAMPLES_PER_FRAME as u32),
        AudioConfig::Opus { .. } => opus_packet_duration(data),
        AudioConfig::Pcma { channels, .. } => {
            ensure!(*channels > 0, "A-law audio without channels");
            Ok((data.len() / *channels as usize) as u32)
        }
        AudioConfig::Mp3 { .. } => mp3_frame_duration(data),
    }
}

/// ISO/IEC 11172-3 §2.4.1.3 and ISO/IEC 13818-3: a Layer III frame holds
/// 1152 samples, 576 in MPEG-2 and 2.5.
fn mp3_frame_duration(frame: &[u8]) -> Result<u32> {
    let &[0xFF, second @ 0xE0..=0xFF, ..] = frame else {
        bail!("MP3 frame does not start with a sync word");
    };
    let version = (second >> 3) & 0x03;
    ensure!(version != 0b01, "MP3 frame has the reserved version");
    ensure!((second >> 1) & 0x03 == 0b01, "MP3 frame is not Layer III");
    Ok(if version == 0b11 { 1_152 } else { 576 })
}

/// RFC 6716 §3.1: the TOC byte's configuration number gives the frame size
/// and its code the frame count; code 3 stores the count in the next byte.
fn opus_packet_duration(packet: &[u8]) -> Result<u32> {
    let toc = *packet.first().context("empty Opus packet")?;
    let configuration = toc >> 3;
    let frame_ticks = match configuration {
        0..=11 => [480, 960, 1_920, 2_880][(configuration % 4) as usize],
        12..=15 => [480, 960][(configuration % 2) as usize],
        _ => [120, 240, 480, 960][(configuration % 4) as usize],
    };
    let frame_count = match toc & 0x03 {
        0 => 1,
        1 | 2 => 2,
        _ => {
            let count = packet
                .get(1)
                .context("Opus code 3 packet without a frame count")?
                & 0x3F;
            ensure!(count > 0, "Opus code 3 packet with zero frames");
            count as u32
        }
    };
    Ok(frame_ticks * frame_count)
}

fn moof_mdat(
    sequence_number: u32,
    track_id: u32,
    decode_time: u64,
    duration: u32,
    sample_flags: u32,
    composition_offset: i32,
    data: &[u8],
) -> Bytes {
    let mut traf = BytesMut::new();
    traf.put_slice(&full_box(
        b"tfhd",
        0,
        TFHD_DEFAULT_BASE_IS_MOOF,
        &track_id.to_be_bytes(),
    ));
    traf.put_slice(&full_box(b"tfdt", 1, 0, &decode_time.to_be_bytes()));
    let mut trun = BytesMut::new();
    trun.put_u32(1);
    trun.put_i32(0);
    trun.put_u32(duration);
    trun.put_u32(data.len() as u32);
    trun.put_u32(sample_flags);
    trun.put_i32(composition_offset);
    let trun_without_offset = full_box(b"trun", 1, TRUN_FLAGS, &trun);
    let mfhd = full_box(b"mfhd", 0, 0, &sequence_number.to_be_bytes());
    let moof_size = 8 + mfhd.len() + 8 + traf.len() + trun_without_offset.len();
    let data_offset = (moof_size + 8) as i32;
    trun[4..8].copy_from_slice(&data_offset.to_be_bytes());
    traf.put_slice(&full_box(b"trun", 1, TRUN_FLAGS, &trun));

    let mut moof_payload = BytesMut::from(mfhd.as_ref());
    moof_payload.put_slice(&plain_box(b"traf", &traf));
    let mut out = BytesMut::from(plain_box(b"moof", &moof_payload).as_ref());
    out.put_slice(&plain_box(b"mdat", data));
    out.freeze()
}

pub(super) fn video_trak(track_id: u32, config: &AvcDecoderConfigurationRecord) -> Result<Bytes> {
    let sps = config.sequence_parameter_set()?;
    let mut entry = BytesMut::new();
    entry.put_bytes(0, 6);
    entry.put_u16(1);
    entry.put_bytes(0, 16);
    entry.put_u16(sps.width as u16);
    entry.put_u16(sps.height as u16);
    entry.put_u32(0x0048_0000);
    entry.put_u32(0x0048_0000);
    entry.put_u32(0);
    entry.put_u16(1);
    entry.put_bytes(0, 32);
    entry.put_u16(0x0018);
    entry.put_i16(-1);
    entry.put_slice(&plain_box(b"avcC", &config.to_bytes()));
    let vmhd = full_box(b"vmhd", 0, 1, &[0; 8]);
    Ok(trak(
        tkhd(track_id, 0, sps.width, sps.height),
        mdhd(VIDEO_TIMESCALE),
        hdlr(b"vide", "VideoHandler"),
        vmhd,
        stbl(&plain_box(b"avc1", &entry)),
    ))
}

pub(super) fn audio_trak(track_id: u32, config: &AudioConfig) -> Bytes {
    let timescale = audio_timescale(config);
    let sample_entry = match config {
        AudioConfig::Aac(config) => plain_box(
            b"mp4a",
            &audio_sample_entry(
                config.channel_count(),
                16,
                timescale,
                &esds(AAC_OBJECT_TYPE_INDICATION, Some(&config.to_bytes())),
            ),
        ),
        AudioConfig::Opus {
            sample_rate,
            channels,
        } => plain_box(
            b"Opus",
            &audio_sample_entry(*channels, 16, timescale, &dops(*sample_rate, *channels)),
        ),
        // ISO BMFF has no MPEG-4 object type for G.711, so this writes the
        // QuickTime `alaw` sample entry, which ffmpeg and VLC read.
        AudioConfig::Pcma { channels, .. } => {
            plain_box(b"alaw", &audio_sample_entry(*channels, 8, timescale, &[]))
        }
        // MPEG-1 audio covers 32 kHz and up; lower rates are MPEG-2 LSF.
        AudioConfig::Mp3 {
            sample_rate,
            channels,
        } => {
            let object_type_indication = if *sample_rate >= 32_000 {
                MPEG1_AUDIO_OBJECT_TYPE_INDICATION
            } else {
                MPEG2_AUDIO_OBJECT_TYPE_INDICATION
            };
            plain_box(
                b"mp4a",
                &audio_sample_entry(
                    *channels,
                    16,
                    timescale,
                    &esds(object_type_indication, None),
                ),
            )
        }
    };
    let smhd = full_box(b"smhd", 0, 0, &[0; 4]);
    trak(
        tkhd(track_id, 0x0100, 0, 0),
        mdhd(timescale),
        hdlr(b"soun", "SoundHandler"),
        smhd,
        stbl(&sample_entry),
    )
}

/// ISO/IEC 14496-12 §12.2.3 AudioSampleEntry followed by the codec's boxes.
fn audio_sample_entry(channels: u8, sample_size: u16, sample_rate: u32, children: &[u8]) -> Bytes {
    let mut entry = BytesMut::new();
    entry.put_bytes(0, 6);
    entry.put_u16(1);
    entry.put_bytes(0, 8);
    entry.put_u16(channels as u16);
    entry.put_u16(sample_size);
    entry.put_u32(0);
    entry.put_u32(sample_rate << 16);
    entry.put_slice(children);
    entry.freeze()
}

/// OpusSpecificBox (Opus-in-ISOBMFF binding §4.3.2): no pre-skip or output
/// gain is known here, and channel mapping family 0 covers mono and stereo.
fn dops(input_sample_rate: u32, channels: u8) -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_u8(0);
    payload.put_u8(channels);
    payload.put_u16(0);
    payload.put_u32(input_sample_rate);
    payload.put_i16(0);
    payload.put_u8(0);
    plain_box(b"dOps", &payload)
}

pub(super) fn plain_box(kind: &[u8; 4], payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(8 + payload.len());
    out.put_u32((8 + payload.len()) as u32);
    out.put_slice(kind);
    out.put_slice(payload);
    out.freeze()
}

pub(super) fn full_box(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Bytes {
    let mut body = BytesMut::with_capacity(4 + payload.len());
    body.put_u8(version);
    body.put_slice(&flags.to_be_bytes()[1..]);
    body.put_slice(payload);
    plain_box(kind, &body)
}

fn ftyp() -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_slice(b"isom");
    payload.put_u32(0x200);
    for brand in [b"isom", b"iso6", b"avc1", b"mp41"] {
        payload.put_slice(brand);
    }
    plain_box(b"ftyp", &payload)
}

fn mvhd(next_track_id: u32) -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_u32(0);
    payload.put_u32(0);
    payload.put_u32(MOVIE_TIMESCALE);
    payload.put_u32(0);
    payload.put_u32(0x0001_0000);
    payload.put_u16(0x0100);
    payload.put_bytes(0, 10);
    for value in UNITY_MATRIX {
        payload.put_u32(value);
    }
    payload.put_bytes(0, 24);
    payload.put_u32(next_track_id);
    full_box(b"mvhd", 0, 0, &payload)
}

fn tkhd(track_id: u32, volume: u16, width: u32, height: u32) -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_u32(0);
    payload.put_u32(0);
    payload.put_u32(track_id);
    payload.put_u32(0);
    payload.put_u32(0);
    payload.put_bytes(0, 8);
    payload.put_u16(0);
    payload.put_u16(0);
    payload.put_u16(volume);
    payload.put_u16(0);
    for value in UNITY_MATRIX {
        payload.put_u32(value);
    }
    payload.put_u32(width << 16);
    payload.put_u32(height << 16);
    full_box(b"tkhd", 0, 3, &payload)
}

fn mdhd(timescale: u32) -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_u32(0);
    payload.put_u32(0);
    payload.put_u32(timescale);
    payload.put_u32(0);
    payload.put_u16(0x55C4);
    payload.put_u16(0);
    full_box(b"mdhd", 0, 0, &payload)
}

fn hdlr(handler_type: &[u8; 4], name: &str) -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_u32(0);
    payload.put_slice(handler_type);
    payload.put_bytes(0, 12);
    payload.put_slice(name.as_bytes());
    payload.put_u8(0);
    full_box(b"hdlr", 0, 0, &payload)
}

fn dinf() -> Bytes {
    let url = full_box(b"url ", 0, 1, &[]);
    let mut dref_payload = BytesMut::new();
    dref_payload.put_u32(1);
    dref_payload.put_slice(&url);
    plain_box(b"dinf", &full_box(b"dref", 0, 0, &dref_payload))
}

fn stbl(sample_entry: &[u8]) -> Bytes {
    let mut stsd_payload = BytesMut::new();
    stsd_payload.put_u32(1);
    stsd_payload.put_slice(sample_entry);
    let mut payload = BytesMut::from(full_box(b"stsd", 0, 0, &stsd_payload).as_ref());
    payload.put_slice(&full_box(b"stts", 0, 0, &0_u32.to_be_bytes()));
    payload.put_slice(&full_box(b"stsc", 0, 0, &0_u32.to_be_bytes()));
    payload.put_slice(&full_box(b"stsz", 0, 0, &[0; 8]));
    payload.put_slice(&full_box(b"stco", 0, 0, &0_u32.to_be_bytes()));
    plain_box(b"stbl", &payload)
}

fn trak(tkhd: Bytes, mdhd: Bytes, hdlr: Bytes, media_header: Bytes, stbl: Bytes) -> Bytes {
    let mut minf = BytesMut::from(media_header.as_ref());
    minf.put_slice(&dinf());
    minf.put_slice(&stbl);
    let mut mdia = BytesMut::from(mdhd.as_ref());
    mdia.put_slice(&hdlr);
    mdia.put_slice(&plain_box(b"minf", &minf));
    let mut payload = BytesMut::from(tkhd.as_ref());
    payload.put_slice(&plain_box(b"mdia", &mdia));
    plain_box(b"trak", &payload)
}

fn descriptor(tag: u8, payload: &[u8]) -> Bytes {
    let mut out = BytesMut::new();
    out.put_u8(tag);
    let mut size = payload.len();
    let mut size_bytes = vec![(size & 0x7F) as u8];
    size >>= 7;
    while size > 0 {
        size_bytes.push(0x80 | (size & 0x7F) as u8);
        size >>= 7;
    }
    size_bytes.reverse();
    out.put_slice(&size_bytes);
    out.put_slice(payload);
    out.freeze()
}

/// ISO/IEC 14496-1 §7.2.6.6: stream type 0x05 (audio) with the upstream flag
/// clear and the reserved bit set.
fn esds(object_type_indication: u8, decoder_specific_info: Option<&[u8]>) -> Bytes {
    let mut decoder_config = BytesMut::new();
    decoder_config.put_u8(object_type_indication);
    decoder_config.put_u8(0x15);
    decoder_config.put_bytes(0, 3);
    decoder_config.put_u32(0);
    decoder_config.put_u32(0);
    if let Some(info) = decoder_specific_info {
        decoder_config.put_slice(&descriptor(0x05, info));
    }
    let mut es = BytesMut::new();
    es.put_u16(0);
    es.put_u8(0);
    es.put_slice(&descriptor(0x04, &decoder_config));
    es.put_slice(&descriptor(0x06, &[0x02]));
    full_box(b"esds", 0, 0, &descriptor(0x03, &es))
}

fn trex(track_id: u32) -> Bytes {
    let mut payload = BytesMut::new();
    payload.put_u32(track_id);
    payload.put_u32(1);
    payload.put_u32(0);
    payload.put_u32(0);
    payload.put_u32(0);
    full_box(b"trex", 0, 0, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mp4::muxer::tests::boxes, sample::Timestamp};

    fn child<'a>(payload: &'a [u8], path: &[&str]) -> &'a [u8] {
        path.iter().fold(payload, |payload, kind| {
            boxes(payload)
                .into_iter()
                .find(|(name, _)| name == kind)
                .unwrap_or_else(|| panic!("missing box {kind}"))
                .1
        })
    }

    /// `stsd` is a full box: skip version/flags and the entry count (8 bytes)
    /// to reach the first entry.
    fn sample_entry(trak: &[u8]) -> (String, Vec<u8>) {
        let stsd = child(trak, &["mdia", "minf", "stbl", "stsd"]);
        let (kind, payload) = boxes(&stsd[8..]).remove(0);
        (kind, payload.to_vec())
    }

    fn mdhd_timescale(trak: &[u8]) -> u32 {
        let mdhd = child(trak, &["mdia", "mdhd"]);
        u32::from_be_bytes(mdhd[12..16].try_into().unwrap())
    }

    #[test]
    fn opus_track_declares_48k_samples_and_describes_the_encoder_in_dops() {
        // Arrange / Act
        let config = AudioConfig::Opus {
            sample_rate: 24_000,
            channels: 2,
        };
        let trak = audio_trak(2, &config);
        let trak = child(&trak, &["trak"]);

        // Assert: the entry runs at 48 kHz while dOps keeps the 24 kHz input rate
        assert_eq!(mdhd_timescale(trak), 48_000);
        let (kind, entry) = sample_entry(trak);
        assert_eq!(kind, "Opus");
        assert_eq!(&entry[16..18], 2_u16.to_be_bytes());
        assert_eq!(&entry[24..28], (48_000_u32 << 16).to_be_bytes());
        let dops = child(&entry[28..], &["dOps"]);
        assert_eq!(dops, [0, 2, 0, 0, 0, 0, 0x5D, 0xC0, 0, 0, 0]);
    }

    #[test]
    fn alaw_track_uses_the_quicktime_entry_with_8_bit_samples() {
        // Arrange / Act
        let config = AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 1,
        };
        let trak = audio_trak(2, &config);
        let trak = child(&trak, &["trak"]);

        // Assert
        assert_eq!(mdhd_timescale(trak), 8_000);
        let (kind, entry) = sample_entry(trak);
        assert_eq!(kind, "alaw");
        assert_eq!(&entry[16..18], 1_u16.to_be_bytes());
        assert_eq!(&entry[18..20], 8_u16.to_be_bytes());
        assert_eq!(&entry[24..28], (8_000_u32 << 16).to_be_bytes());
        assert_eq!(entry.len(), 28);
    }

    #[test]
    fn mp3_track_is_an_mp4a_entry_with_the_mpeg_audio_object_type() {
        // Arrange: (sample rate, objectTypeIndication)
        let cases = [(44_100, 0x6B), (22_050, 0x69)];

        for (sample_rate, object_type_indication) in cases {
            // Act
            let config = AudioConfig::Mp3 {
                sample_rate,
                channels: 2,
            };
            let trak = audio_trak(2, &config);
            let trak = child(&trak, &["trak"]);

            // Assert: ES_Descriptor (3 bytes) then the DecoderConfigDescriptor tag, size and OTI
            assert_eq!(mdhd_timescale(trak), sample_rate);
            let (kind, entry) = sample_entry(trak);
            assert_eq!(kind, "mp4a");
            let esds = child(&entry[28..], &["esds"]);
            let es = &esds[4..];
            assert_eq!(es[0], 0x03);
            let decoder_config = &es[2 + 3..];
            assert_eq!(decoder_config[0], 0x04);
            assert_eq!(decoder_config[2], object_type_indication, "{sample_rate}");
        }
    }

    #[test]
    fn mp3_frame_duration_follows_the_version_bits() {
        // Arrange: (second header byte, expected samples) for MPEG-1, 2 and 2.5 Layer III
        let cases = [(0xFB, 1_152), (0xF3, 576), (0xE3, 576)];

        // Act / Assert
        for (second, expected) in cases {
            assert_eq!(
                mp3_frame_duration(&[0xFF, second, 0x90, 0x64]).unwrap(),
                expected
            );
        }
        assert!(mp3_frame_duration(&[0x00, 0xFB]).is_err());
        assert!(mp3_frame_duration(&[0xFF, 0xE9]).is_err());
        assert!(mp3_frame_duration(&[0xFF, 0xFD]).is_err());
        assert!(mp3_frame_duration(&[0xFF]).is_err());
    }

    #[test]
    fn opus_packet_duration_follows_the_toc_byte() {
        // Arrange: (packet, expected 48 kHz ticks)
        let cases: [(&[u8], u32); 5] = [
            (&[0x00, 1, 2], 480),         // SILK 10 ms, one frame
            (&[0x68, 1, 2], 960),         // Hybrid 20 ms, one frame
            (&[0x81, 1, 2], 240),         // CELT 2.5 ms, two frames
            (&[0xFA, 1, 2], 1_920),       // CELT 20 ms, two frames
            (&[0x0B, 0x03, 1, 2], 2_880), // SILK 20 ms, code 3 with three frames
        ];

        // Act / Assert
        for (packet, expected) in cases {
            assert_eq!(
                opus_packet_duration(packet).unwrap(),
                expected,
                "{packet:?}"
            );
        }
        assert!(opus_packet_duration(&[]).is_err());
        assert!(opus_packet_duration(&[0x0B, 0x00]).is_err());
    }

    #[test]
    fn alaw_fragment_lasts_one_tick_per_sample_of_each_channel() {
        // Arrange
        let config = AudioConfig::Pcma {
            sample_rate: 8_000,
            channels: 2,
        };
        let sample = AudioSample {
            data: Bytes::from(vec![0xD5; 320]),
            pts: Timestamp::from_millis(20),
        };

        // Act
        let fragment = audio_fragment(1, 2, &sample, &config).unwrap();

        // Assert
        let trun = child(&fragment, &["moof", "traf", "trun"]);
        assert_eq!(&trun[12..16], 160_u32.to_be_bytes());
        let tfdt = child(&fragment, &["moof", "traf", "tfdt"]);
        assert_eq!(&tfdt[4..12], 160_u64.to_be_bytes());
    }

    #[test]
    fn encodes_multi_byte_descriptor_sizes() {
        // Arrange
        let payload = vec![0xAB; 300];

        // Act
        let encoded = descriptor(0x05, &payload);

        // Assert
        assert_eq!(&encoded[..3], [0x05, 0x82, 0x2C]);
        assert_eq!(encoded.len(), 303);
    }
}
