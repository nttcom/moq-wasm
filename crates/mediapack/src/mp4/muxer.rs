use anyhow::{Context, Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{
    mp4::isobmff::{self, TrackInit, VIDEO_TIMESCALE},
    sample::{AudioConfig, MediaEvent, StreamSet, VideoConfig, VideoSample},
};

const VIDEO_TRACK_ID: u32 = 1;
const AUDIO_TRACK_ID: u32 = 2;

#[derive(Default)]
pub struct Fmp4Muxer {
    expected: StreamSet,
    video_config: Option<VideoConfig>,
    audio_config: Option<AudioConfig>,
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
        let duration = self.last_video_duration;
        self.video_fragment(&sample, duration)
    }

    fn configs_ready(&self) -> bool {
        (!self.expected.has_video || self.video_config.is_some())
            && (!self.expected.has_audio || self.audio_config.is_some())
    }

    fn fragment(&mut self, event: &MediaEvent) -> Result<Bytes> {
        match event {
            MediaEvent::Video(sample) => {
                let Some(previous) = self.pending_video.replace(sample.clone()) else {
                    return Ok(Bytes::new());
                };
                let duration = sample
                    .dts
                    .saturating_sub(previous.dts)
                    .ticks(VIDEO_TIMESCALE) as u32;
                self.last_video_duration = duration;
                self.video_fragment(&previous, duration)
            }
            MediaEvent::Audio(sample) => {
                let config = self
                    .audio_config
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("audio sample without configuration"))?;
                self.sequence_number += 1;
                isobmff::audio_fragment(self.sequence_number, AUDIO_TRACK_ID, sample, config)
            }
            _ => Ok(Bytes::new()),
        }
    }

    fn video_fragment(&mut self, sample: &VideoSample, duration: u32) -> Result<Bytes> {
        let config = self
            .video_config
            .as_ref()
            .context("video sample without configuration")?;
        self.sequence_number += 1;
        isobmff::video_fragment(
            self.sequence_number,
            VIDEO_TRACK_ID,
            sample,
            duration,
            config,
        )
    }

    fn init_segment(&self) -> Result<Bytes> {
        let mut tracks = Vec::new();
        if let Some(config) = &self.video_config {
            tracks.push(TrackInit {
                track_id: VIDEO_TRACK_ID,
                trak: isobmff::video_trak(VIDEO_TRACK_ID, config)?,
            });
        }
        if let Some(config) = &self.audio_config {
            tracks.push(TrackInit {
                track_id: AUDIO_TRACK_ID,
                trak: isobmff::audio_trak(AUDIO_TRACK_ID, config),
            });
        }
        ensure!(
            !tracks.is_empty(),
            "init segment needs at least one configured track"
        );
        Ok(isobmff::init_segment(&tracks))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        aac::AudioSpecificConfig,
        loc,
        mp4::isobmff::{NON_SYNC_SAMPLE_FLAGS, SYNC_SAMPLE_FLAGS},
        sample::{AudioConfig, AudioSample, Timestamp},
        test_support::{
            FIXTURE_AV1_IVF, FIXTURE_VP8_IVF, FIXTURE_VP9_IVF, delta_frame_annexb,
            fixture_video_config, ivf_frames, keyframe_annexb, mono_48k,
        },
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
            .push(&MediaEvent::VideoConfig(fixture_video_config()))
            .unwrap();
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();
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
            .push(&MediaEvent::VideoConfig(fixture_video_config()))
            .unwrap();

        // Act
        let before_audio_config = muxer.push(&video(0, true)).unwrap();
        muxer
            .push(&MediaEvent::AudioConfig(AudioConfig::Aac(mono_48k())))
            .unwrap();
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
        let result = muxer.push(&MediaEvent::AudioConfig(AudioConfig::Aac(changed)));

        // Assert
        assert!(result.is_err());
    }

    /// A video-only file of the fixture's two frames, demuxed from LOC objects.
    fn mux_ivf(codec: loc::VideoCodec, fixture: &[u8]) -> Vec<u8> {
        let mut demuxer = loc::Demuxer::video(codec);
        let mut muxer = Fmp4Muxer::new();
        let mut file = Vec::new();
        for (index, payload) in ivf_frames(fixture).into_iter().enumerate() {
            let object = loc::LocObject {
                extensions: vec![loc::LocExtension::varint(
                    loc::CAPTURE_TIMESTAMP_ID,
                    index as u64 * 33_333,
                )],
                payload,
            };
            for event in demuxer.push(&object).unwrap() {
                file.extend_from_slice(&muxer.push(&event).unwrap());
            }
        }
        file.extend_from_slice(&muxer.finish().unwrap());
        file
    }

    /// The payload of the first box of `kind` at any depth.
    fn nested_box<'a>(data: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
        let start = data
            .windows(4)
            .position(|window| window == kind)
            .unwrap_or_else(|| panic!("missing box {}", String::from_utf8_lossy(kind)));
        let size = u32::from_be_bytes(data[start - 4..start].try_into().unwrap()) as usize;
        &data[start + 4..start - 4 + size]
    }

    #[test]
    fn writes_vp8_and_vp9_with_a_vp_codec_configuration() {
        // Act
        let vp8 = mux_ivf(loc::VideoCodec::Vp8, FIXTURE_VP8_IVF);
        let vp9 = mux_ivf(loc::VideoCodec::Vp9, FIXTURE_VP9_IVF);

        // Assert: version 1, profile 0, level 1, 8-bit 4:2:0 limited range, colour unspecified
        let expected_vpcc = [1, 0, 0, 0, 0, 10, 0x82, 2, 2, 2, 0, 0];
        assert_eq!(nested_box(&vp8, b"vpcC"), expected_vpcc);
        assert_eq!(nested_box(&vp9, b"vpcC"), expected_vpcc);
        assert_eq!(
            nested_box(&vp9, b"mdat"),
            ivf_frames(FIXTURE_VP9_IVF)[0].as_ref()
        );
    }

    #[test]
    fn writes_av1_samples_without_their_temporal_delimiter() {
        // Arrange
        let frames = ivf_frames(FIXTURE_AV1_IVF);

        // Act
        let file = mux_ivf(loc::VideoCodec::Av1, FIXTURE_AV1_IVF);

        // Assert: marker and version, profile 0 level 0, 8-bit 4:2:0, then the sequence header OBU
        let av1c = nested_box(&file, b"av1C");
        assert_eq!(av1c[..4], [0x81, 0x00, 0x0C, 0x00]);
        assert_eq!(av1c[4..], frames[0][2..14]);
        assert_eq!(nested_box(&file, b"mdat"), &frames[0][2..]);
    }
}
