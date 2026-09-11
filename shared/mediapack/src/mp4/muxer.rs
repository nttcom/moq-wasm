use anyhow::{Result, ensure};
use bytes::{BufMut, Bytes, BytesMut};

use crate::{
    aac::AudioSpecificConfig,
    h264::AvcDecoderConfigurationRecord,
    mp4::isobmff::{self, TrackInit, VIDEO_TIMESCALE},
    sample::{MediaEvent, StreamSet, VideoSample},
};

const VIDEO_TRACK_ID: u32 = 1;
const AUDIO_TRACK_ID: u32 = 2;

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
                let timescale = self
                    .audio_config
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("audio sample without configuration"))?
                    .sample_rate;
                self.sequence_number += 1;
                Ok(isobmff::audio_fragment(
                    self.sequence_number,
                    AUDIO_TRACK_ID,
                    sample,
                    timescale,
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
        self.sequence_number += 1;
        isobmff::video_fragment(
            self.sequence_number,
            VIDEO_TRACK_ID,
            sample,
            duration,
            nal_length_size,
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
        mp4::isobmff::{NON_SYNC_SAMPLE_FLAGS, SYNC_SAMPLE_FLAGS},
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
}
