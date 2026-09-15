use anyhow::{Result, bail, ensure};
use bytes::Bytes;

use crate::{
    aac::AudioSpecificConfig,
    h264::AvcDecoderConfigurationRecord,
    mp4::isobmff::{self, TrackInit, VIDEO_TIMESCALE},
    sample::{MediaEvent, VideoSample},
};

const TRACK_ID: u32 = 1;

/// One sample of one track as a `moof` followed by its `mdat`, which is the
/// unit draft-ietf-moq-cmsf-01 §3.3 requires a MoQT object to carry.
pub struct Fragment {
    pub data: Bytes,
    pub is_keyframe: bool,
}

/// Muxes a single track so the init segment and the fragments travel apart:
/// draft-ietf-moq-cmsf-01 §3.1 places the init segment in the catalog and §3.3
/// gives each fragment its own MoQT object.
pub struct Fmp4TrackMuxer {
    track: Track,
    sequence_number: u32,
    pending_video: Option<VideoSample>,
    last_video_duration: u32,
}

enum Track {
    Video(AvcDecoderConfigurationRecord),
    Audio(AudioSpecificConfig),
}

impl Fmp4TrackMuxer {
    pub fn video(config: AvcDecoderConfigurationRecord) -> Self {
        Self::new(Track::Video(config))
    }

    pub fn audio(config: AudioSpecificConfig) -> Self {
        Self::new(Track::Audio(config))
    }

    fn new(track: Track) -> Self {
        Self {
            track,
            sequence_number: 0,
            pending_video: None,
            last_video_duration: 0,
        }
    }

    pub fn init_segment(&self) -> Result<Bytes> {
        let trak = match &self.track {
            Track::Video(config) => isobmff::video_trak(TRACK_ID, config)?,
            Track::Audio(config) => isobmff::audio_trak(TRACK_ID, config),
        };
        Ok(isobmff::init_segment(&[TrackInit {
            track_id: TRACK_ID,
            trak,
        }]))
    }

    /// A video sample is held back until the next one arrives, because its
    /// duration is the gap between the two decode times.
    pub fn push(&mut self, event: &MediaEvent) -> Result<Option<Fragment>> {
        match event {
            MediaEvent::Video(sample) => {
                let Track::Video(config) = &self.track else {
                    bail!("video sample pushed into an audio track");
                };
                let nal_length_size = config.nal_length_size as usize;
                let Some(previous) = self.pending_video.replace(sample.clone()) else {
                    return Ok(None);
                };
                let duration = sample
                    .dts
                    .saturating_sub(previous.dts)
                    .ticks(VIDEO_TIMESCALE) as u32;
                self.last_video_duration = duration;
                Ok(Some(self.video_fragment(
                    &previous,
                    duration,
                    nal_length_size,
                )))
            }
            MediaEvent::Audio(sample) => {
                let Track::Audio(config) = &self.track else {
                    bail!("audio sample pushed into a video track");
                };
                let timescale = config.sample_rate;
                self.sequence_number += 1;
                Ok(Some(Fragment {
                    data: isobmff::audio_fragment(
                        self.sequence_number,
                        TRACK_ID,
                        sample,
                        timescale,
                    ),
                    is_keyframe: true,
                }))
            }
            MediaEvent::VideoConfig(update) => {
                let Track::Video(config) = &self.track else {
                    bail!("video configuration pushed into an audio track");
                };
                ensure!(
                    config == update,
                    "video configuration changed after the init segment"
                );
                Ok(None)
            }
            MediaEvent::AudioConfig(update) => {
                let Track::Audio(config) = &self.track else {
                    bail!("audio configuration pushed into a video track");
                };
                ensure!(
                    config == update,
                    "audio configuration changed after the init segment"
                );
                Ok(None)
            }
            MediaEvent::Streams(_) => Ok(None),
        }
    }

    pub fn finish(&mut self) -> Option<Fragment> {
        let sample = self.pending_video.take()?;
        let Track::Video(config) = &self.track else {
            return None;
        };
        let nal_length_size = config.nal_length_size as usize;
        let duration = self.last_video_duration;
        Some(self.video_fragment(&sample, duration, nal_length_size))
    }

    fn video_fragment(
        &mut self,
        sample: &VideoSample,
        duration: u32,
        nal_length_size: usize,
    ) -> Fragment {
        self.sequence_number += 1;
        Fragment {
            data: isobmff::video_fragment(
                self.sequence_number,
                TRACK_ID,
                sample,
                duration,
                nal_length_size,
            ),
            is_keyframe: sample.is_keyframe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mp4::muxer::tests::boxes,
        sample::{AudioSample, Timestamp},
        test_support::{delta_frame_annexb, fixture_record, keyframe_annexb, mono_48k},
    };

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

    fn kinds(data: &[u8]) -> Vec<String> {
        boxes(data).into_iter().map(|(kind, _)| kind).collect()
    }

    fn traf_track_id(fragment: &[u8]) -> u32 {
        let top = boxes(fragment);
        let moof = boxes(top.iter().find(|(kind, _)| kind == "moof").unwrap().1);
        let traf = boxes(moof.iter().find(|(kind, _)| kind == "traf").unwrap().1);
        let tfhd = traf.iter().find(|(kind, _)| kind == "tfhd").unwrap().1;
        u32::from_be_bytes(tfhd[4..8].try_into().unwrap())
    }

    #[test]
    fn init_segment_describes_exactly_one_track() {
        // Arrange
        let muxer = Fmp4TrackMuxer::video(fixture_record());

        // Act
        let init = muxer.init_segment().unwrap();

        // Assert
        assert_eq!(kinds(&init), ["ftyp", "moov"]);
        let moov = boxes(boxes(&init)[1].1);
        assert_eq!(moov.iter().filter(|(kind, _)| kind == "trak").count(), 1);
        let mvex = moov.iter().find(|(kind, _)| kind == "mvex").unwrap().1;
        assert_eq!(boxes(mvex).len(), 1);
    }

    #[test]
    fn video_fragments_lag_one_sample_and_flag_the_keyframe() {
        // Arrange
        let mut muxer = Fmp4TrackMuxer::video(fixture_record());

        // Act
        let first = muxer.push(&video(0, true)).unwrap();
        let second = muxer.push(&video(40, false)).unwrap().unwrap();
        let flushed = muxer.finish().unwrap();

        // Assert
        assert!(first.is_none());
        assert!(second.is_keyframe);
        assert_eq!(kinds(&second.data), ["moof", "mdat"]);
        assert_eq!(traf_track_id(&second.data), TRACK_ID);
        assert!(!flushed.is_keyframe);
        assert_eq!(kinds(&flushed.data), ["moof", "mdat"]);
    }

    #[test]
    fn audio_fragments_are_emitted_per_sample() {
        // Arrange
        let mut muxer = Fmp4TrackMuxer::audio(mono_48k());

        // Act
        let fragment = muxer.push(&audio(0)).unwrap().unwrap();

        // Assert
        assert!(fragment.is_keyframe);
        assert_eq!(kinds(&fragment.data), ["moof", "mdat"]);
        assert_eq!(traf_track_id(&fragment.data), TRACK_ID);
        assert_eq!(boxes(&fragment.data)[1].1, [0xDE, 0xAD]);
        assert!(muxer.finish().is_none());
    }

    #[test]
    fn rejects_samples_of_the_other_track_kind() {
        // Arrange
        let mut video_muxer = Fmp4TrackMuxer::video(fixture_record());
        let mut audio_muxer = Fmp4TrackMuxer::audio(mono_48k());

        // Act / Assert
        assert!(video_muxer.push(&audio(0)).is_err());
        assert!(audio_muxer.push(&video(0, true)).is_err());
    }
}
