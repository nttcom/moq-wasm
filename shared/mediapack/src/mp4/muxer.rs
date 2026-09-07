use anyhow::{Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{
    aac::{AudioSpecificConfig, asc::SAMPLES_PER_FRAME},
    h264::{AvcDecoderConfigurationRecord, annexb::annexb_to_avcc_without_parameter_sets},
    sample::{MediaEvent, StreamSet, VideoSample},
};

const VIDEO_TRACK_ID: u32 = 1;
const AUDIO_TRACK_ID: u32 = 2;
const VIDEO_TIMESCALE: u32 = 90_000;
const MOVIE_TIMESCALE: u32 = 1_000;
const SYNC_SAMPLE_FLAGS: u32 = 0x0200_0000;
const NON_SYNC_SAMPLE_FLAGS: u32 = 0x0101_0000;
const TFHD_DEFAULT_BASE_IS_MOOF: u32 = 0x0002_0000;
const TRUN_FLAGS: u32 = 0x0001 | 0x0100 | 0x0200 | 0x0400 | 0x0800;

#[derive(Default)]
pub struct Fmp4Muxer {
    expected: StreamSet,
    video_config: Option<AvcDecoderConfigurationRecord>,
    audio_config: Option<AudioSpecificConfig>,
    init_written: bool,
    sequence_number: u32,
    buffered: Vec<MediaEvent>,
    pending_video: Option<VideoSample>,
    last_video_duration: u32,
}

impl Fmp4Muxer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: &MediaEvent) -> Result<Bytes> {
        match event {
            MediaEvent::Streams(streams) => {
                self.expected = *streams;
                Ok(Bytes::new())
            }
            MediaEvent::VideoConfig(config) => {
                if self.init_written {
                    ensure!(
                        self.video_config.as_ref() == Some(config),
                        "video configuration changed after the init segment was written"
                    );
                }
                self.video_config = Some(config.clone());
                Ok(Bytes::new())
            }
            MediaEvent::AudioConfig(config) => {
                if self.init_written {
                    ensure!(
                        self.audio_config.as_ref() == Some(config),
                        "audio configuration changed after the init segment was written"
                    );
                }
                self.audio_config = Some(config.clone());
                Ok(Bytes::new())
            }
            MediaEvent::Video(_) | MediaEvent::Audio(_) => {
                if self.init_written {
                    return self.fragment(event);
                }
                self.buffered.push(event.clone());
                if !self.configs_ready() {
                    return Ok(Bytes::new());
                }
                let mut out = BytesMut::from(self.init_segment()?.as_ref());
                self.init_written = true;
                for buffered in std::mem::take(&mut self.buffered) {
                    out.put_slice(&self.fragment(&buffered)?);
                }
                Ok(out.freeze())
            }
        }
    }

    pub fn finish(&mut self) -> Result<Bytes> {
        let Some(sample) = self.pending_video.take() else {
            return Ok(Bytes::new());
        };
        let nal_length_size = self
            .video_config
            .as_ref()
            .map_or(4, |config| config.nal_length_size as usize);
        let duration = self.last_video_duration;
        Ok(self.video_fragment(&sample, duration, nal_length_size))
    }

    fn configs_ready(&self) -> bool {
        (!self.expected.has_video || self.video_config.is_some())
            && (!self.expected.has_audio || self.audio_config.is_some())
    }

    fn fragment(&mut self, event: &MediaEvent) -> Result<Bytes> {
        match event {
            MediaEvent::Video(sample) => {
                let nal_length_size = self
                    .video_config
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("video sample without configuration"))?
                    .nal_length_size as usize;
                let Some(previous) = self.pending_video.replace(sample.clone()) else {
                    return Ok(Bytes::new());
                };
                let duration = sample
                    .dts
                    .saturating_sub(previous.dts)
                    .ticks(VIDEO_TIMESCALE) as u32;
                self.last_video_duration = duration;
                Ok(self.video_fragment(&previous, duration, nal_length_size))
            }
            MediaEvent::Audio(sample) => {
                let config = self
                    .audio_config
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("audio sample without configuration"))?;
                let timescale = config.sample_rate;
                Ok(self.moof_mdat(
                    AUDIO_TRACK_ID,
                    sample.pts.ticks(timescale),
                    SAMPLES_PER_FRAME as u32,
                    SYNC_SAMPLE_FLAGS,
                    0,
                    &sample.data,
                ))
            }
            _ => Ok(Bytes::new()),
        }
    }

    fn video_fragment(
        &mut self,
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
        self.moof_mdat(
            VIDEO_TRACK_ID,
            sample.dts.ticks(VIDEO_TIMESCALE),
            duration,
            flags,
            composition_offset as i32,
            &data,
        )
    }

    fn moof_mdat(
        &mut self,
        track_id: u32,
        decode_time: u64,
        duration: u32,
        sample_flags: u32,
        composition_offset: i32,
        data: &[u8],
    ) -> Bytes {
        self.sequence_number += 1;
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
        let mfhd = full_box(b"mfhd", 0, 0, &self.sequence_number.to_be_bytes());
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

    fn init_segment(&self) -> Result<Bytes> {
        let mut moov = BytesMut::from(mvhd().as_ref());
        let mut mvex = BytesMut::new();
        if let Some(config) = &self.video_config {
            moov.put_slice(&video_trak(config)?);
            mvex.put_slice(&trex(VIDEO_TRACK_ID));
        }
        if let Some(config) = &self.audio_config {
            moov.put_slice(&audio_trak(config));
            mvex.put_slice(&trex(AUDIO_TRACK_ID));
        }
        ensure!(
            !mvex.is_empty(),
            "init segment needs at least one configured track"
        );
        moov.put_slice(&plain_box(b"mvex", &mvex));
        let mut out = BytesMut::from(ftyp().as_ref());
        out.put_slice(&plain_box(b"moov", &moov));
        Ok(out.freeze())
    }
}

fn plain_box(kind: &[u8; 4], payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(8 + payload.len());
    out.put_u32((8 + payload.len()) as u32);
    out.put_slice(kind);
    out.put_slice(payload);
    out.freeze()
}

fn full_box(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Bytes {
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

const UNITY_MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn mvhd() -> Bytes {
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
    payload.put_u32(AUDIO_TRACK_ID + 1);
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

fn video_trak(config: &AvcDecoderConfigurationRecord) -> Result<Bytes> {
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
        tkhd(VIDEO_TRACK_ID, 0, sps.width, sps.height),
        mdhd(VIDEO_TIMESCALE),
        hdlr(b"vide", "VideoHandler"),
        vmhd,
        stbl(&plain_box(b"avc1", &entry)),
    ))
}

fn audio_trak(config: &AudioSpecificConfig) -> Bytes {
    let mut entry = BytesMut::new();
    entry.put_bytes(0, 6);
    entry.put_u16(1);
    entry.put_bytes(0, 8);
    entry.put_u16(config.channel_count() as u16);
    entry.put_u16(16);
    entry.put_u32(0);
    entry.put_u32(config.sample_rate << 16);
    entry.put_slice(&esds(config));
    let smhd = full_box(b"smhd", 0, 0, &[0; 4]);
    trak(
        tkhd(AUDIO_TRACK_ID, 0x0100, 0, 0),
        mdhd(config.sample_rate),
        hdlr(b"soun", "SoundHandler"),
        smhd,
        stbl(&plain_box(b"mp4a", &entry)),
    )
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

fn esds(config: &AudioSpecificConfig) -> Bytes {
    let mut decoder_config = BytesMut::new();
    decoder_config.put_u8(0x40);
    decoder_config.put_u8(0x15);
    decoder_config.put_bytes(0, 3);
    decoder_config.put_u32(0);
    decoder_config.put_u32(0);
    decoder_config.put_slice(&descriptor(0x05, &config.to_bytes()));
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
pub(crate) mod tests {
    use super::*;
    use crate::{
        sample::{AudioSample, Timestamp},
        test_support::{delta_frame_annexb, fixture_record, keyframe_annexb, mono_48k},
    };

    pub(crate) fn boxes(data: &[u8]) -> Vec<(String, &[u8])> {
        let mut out = Vec::new();
        let mut offset = 0;
        while offset + 8 <= data.len() {
            let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = String::from_utf8_lossy(&data[offset + 4..offset + 8]).into_owned();
            out.push((kind, &data[offset + 8..offset + size]));
            offset += size;
        }
        assert_eq!(offset, data.len(), "trailing bytes after last box");
        out
    }

    fn find<'a>(boxes: &[(String, &'a [u8])], kind: &str) -> &'a [u8] {
        boxes
            .iter()
            .find(|(name, _)| name == kind)
            .unwrap_or_else(|| panic!("missing box {kind}"))
            .1
    }

    fn video(dts_ms: u64, is_keyframe: bool) -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: if is_keyframe {
                keyframe_annexb()
            } else {
                delta_frame_annexb()
            },
            is_keyframe,
            pts: Timestamp::from_millis(dts_ms + 40),
            dts: Timestamp::from_millis(dts_ms),
        })
    }

    fn audio(pts_ms: u64) -> MediaEvent {
        MediaEvent::Audio(AudioSample {
            data: Bytes::from_static(&[0xDE, 0xAD]),
            pts: Timestamp::from_millis(pts_ms),
        })
    }

    fn muxer_with_both_tracks() -> Fmp4Muxer {
        let mut muxer = Fmp4Muxer::new();
        muxer
            .push(&MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true,
            }))
            .unwrap();
        muxer
            .push(&MediaEvent::VideoConfig(fixture_record()))
            .unwrap();
        muxer.push(&MediaEvent::AudioConfig(mono_48k())).unwrap();
        muxer
    }

    #[test]
    fn writes_init_segment_with_both_tracks_before_first_fragment() {
        // Arrange
        let mut muxer = muxer_with_both_tracks();

        // Act
        let out = muxer.push(&audio(0)).unwrap();

        // Assert
        let top = boxes(&out);
        let kinds: Vec<&str> = top.iter().map(|(kind, _)| kind.as_str()).collect();
        assert_eq!(kinds, ["ftyp", "moov", "moof", "mdat"]);
        let moov = boxes(find(&top, "moov"));
        assert_eq!(moov.iter().filter(|(kind, _)| kind == "trak").count(), 2);
        let mvex = boxes(find(&moov, "mvex"));
        assert_eq!(mvex.len(), 2);
        assert_eq!(find(&top, "mdat"), [0xDE, 0xAD]);
    }

    #[test]
    fn holds_back_video_until_next_sample_defines_duration() {
        // Arrange
        let mut muxer = muxer_with_both_tracks();

        // Act
        let first = muxer.push(&video(0, true)).unwrap();
        let second = muxer.push(&video(40, false)).unwrap();
        let flushed = muxer.finish().unwrap();

        // Assert
        let first_kinds: Vec<String> = boxes(&first).into_iter().map(|(k, _)| k).collect();
        assert_eq!(first_kinds, ["ftyp", "moov"]);
        let second_boxes = boxes(&second);
        let traf = boxes(boxes(find(&second_boxes, "moof"))[1].1);
        let trun = find(&traf, "trun");
        assert_eq!(&trun[12..16], 3_600_u32.to_be_bytes());
        assert_eq!(&trun[20..24], SYNC_SAMPLE_FLAGS.to_be_bytes());
        assert_eq!(&trun[24..28], 3_600_i32.to_be_bytes());
        let tfdt = find(&traf, "tfdt");
        assert_eq!(&tfdt[4..12], 0_u64.to_be_bytes());
        let flushed_boxes = boxes(&flushed);
        let flushed_traf = boxes(boxes(find(&flushed_boxes, "moof"))[1].1);
        let flushed_trun = find(&flushed_traf, "trun");
        assert_eq!(&flushed_trun[12..16], 3_600_u32.to_be_bytes());
        assert_eq!(&flushed_trun[20..24], NON_SYNC_SAMPLE_FLAGS.to_be_bytes());
        assert_eq!(&find(&flushed_traf, "tfdt")[4..12], 3_600_u64.to_be_bytes());
    }

    #[test]
    fn strips_parameter_sets_from_video_sample_data() {
        // Arrange
        let mut muxer = muxer_with_both_tracks();
        muxer.push(&video(0, true)).unwrap();

        // Act
        let out = muxer.push(&video(40, false)).unwrap();

        // Assert
        let mdat = find(&boxes(&out), "mdat");
        assert_eq!(mdat, [0, 0, 0, 3, 0x65, 0x88, 0x84]);
    }

    #[test]
    fn data_offset_points_at_first_mdat_byte() {
        // Arrange
        let mut muxer = muxer_with_both_tracks();

        // Act
        let out = muxer.push(&audio(0)).unwrap();

        // Assert
        let top = boxes(&out);
        let moof_start = 8 + find(&top, "ftyp").len() + 8 + find(&top, "moov").len();
        let moof = find(&top, "moof");
        let traf = boxes(boxes(moof)[1].1);
        let trun = find(&traf, "trun");
        let data_offset = u32::from_be_bytes(trun[8..12].try_into().unwrap()) as usize;
        assert_eq!(moof_start + data_offset, out.len() - 2);
    }

    #[test]
    fn buffers_samples_until_expected_configs_arrive() {
        // Arrange
        let mut muxer = Fmp4Muxer::new();
        muxer
            .push(&MediaEvent::Streams(StreamSet {
                has_video: true,
                has_audio: true,
            }))
            .unwrap();
        muxer
            .push(&MediaEvent::VideoConfig(fixture_record()))
            .unwrap();

        // Act
        let before_audio_config = muxer.push(&video(0, true)).unwrap();
        muxer.push(&MediaEvent::AudioConfig(mono_48k())).unwrap();
        let after = muxer.push(&audio(0)).unwrap();

        // Assert
        assert!(before_audio_config.is_empty());
        let kinds: Vec<String> = boxes(&after).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, ["ftyp", "moov", "moof", "mdat"]);
    }

    #[test]
    fn rejects_configuration_change_after_init() {
        // Arrange
        let mut muxer = muxer_with_both_tracks();
        muxer.push(&audio(0)).unwrap();
        let changed = AudioSpecificConfig::new(2, 48_000, 2);

        // Act
        let result = muxer.push(&MediaEvent::AudioConfig(changed));

        // Assert
        assert!(result.is_err());
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
