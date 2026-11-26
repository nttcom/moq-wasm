use anyhow::{Context, Result};
use bytes::Bytes;
use mediapack::{
    AudioConfig, AudioSample, MediaEvent, Timestamp, aac::AudioSpecificConfig,
    h264::ParameterSetTracker,
};

/// Microseconds of `ticks` counted in units of `timebase_num / timebase_den`
/// seconds. `None` when the timebase is not positive or the result does not
/// fit.
pub(crate) fn ticks_to_micros(ticks: i64, timebase_num: i32, timebase_den: i32) -> Option<i64> {
    if timebase_num <= 0 || timebase_den <= 0 {
        return None;
    }
    let micros = i128::from(ticks) * 1_000_000 * i128::from(timebase_num);
    i64::try_from(micros.div_euclid(i128::from(timebase_den))).ok()
}

/// Longer than OBS holds a frame back for encoding, audio buffering, and its
/// stream delay setting together.
const MAX_OBS_DELAY_MICROS: i64 = 3_600_000_000;

/// How long ago OBS composed the frame it stamped `frame_obs_micros`, on its
/// clock that now reads `obs_now_micros`. `None` when the stamp is in the
/// future or more than an hour old, as for a stamp that is on another clock.
pub(crate) fn obs_delay_micros(obs_now_micros: i64, frame_obs_micros: i64) -> Option<u64> {
    let delay = obs_now_micros.checked_sub(frame_obs_micros)?;
    (0..=MAX_OBS_DELAY_MICROS)
        .contains(&delay)
        .then_some(delay as u64)
}

/// The wall-clock time of presentation time zero, given a frame shown at
/// `presentation_time` that was captured `delay_micros` before `wall_now`.
pub(crate) fn capture_origin(
    wall_now: Timestamp,
    delay_micros: u64,
    presentation_time: Timestamp,
) -> Timestamp {
    wall_now
        .saturating_sub(Timestamp::from_micros(delay_micros))
        .saturating_sub(presentation_time)
}

/// Turns the packets of OBS's H.264 and AAC encoders into `MediaEvent`s on
/// one clock that starts at the first video packet.
#[derive(Default)]
pub(crate) struct MediaInput {
    parameter_sets: ParameterSetTracker,
    /// Microseconds added to every timestamp so that the first video DTS,
    /// which B-frame reordering makes negative, becomes zero.
    shift_micros: Option<i64>,
    audio_configured: bool,
}

impl MediaInput {
    /// `extra_data` is the encoder's SPS and PPS in Annex B: x264 sends them
    /// only there, not with its keyframes. Timestamps are microseconds.
    pub(crate) fn video(
        &mut self,
        extra_data: &[u8],
        annexb: &[u8],
        pts: i64,
        dts: i64,
    ) -> Result<Vec<MediaEvent>> {
        let shift = match self.shift_micros {
            Some(shift) => shift,
            None => {
                self.parameter_sets.track(extra_data)?;
                *self.shift_micros.insert(dts.saturating_neg())
            }
        };
        let Some(unit) = self.parameter_sets.track(annexb)? else {
            return Ok(Vec::new());
        };
        Ok(unit.into_events(shifted(pts, shift), shifted(dts, shift)))
    }

    /// `extra_data` is the encoder's AudioSpecificConfig. Audio that comes
    /// before the first video packet has no place on the clock and is left
    /// out.
    pub(crate) fn audio(
        &mut self,
        extra_data: &[u8],
        frame: &[u8],
        pts: i64,
    ) -> Result<Vec<MediaEvent>> {
        let Some(shift) = self.shift_micros else {
            return Ok(Vec::new());
        };
        let mut events = Vec::new();
        if !self.audio_configured {
            let config = AudioSpecificConfig::parse(extra_data)
                .context("parse the AudioSpecificConfig of the AAC encoder")?;
            events.push(MediaEvent::AudioConfig(AudioConfig::Aac(config)));
            self.audio_configured = true;
        }
        events.push(MediaEvent::Audio(AudioSample {
            data: Bytes::copy_from_slice(frame),
            pts: shifted(pts, shift),
            audio_level: None,
        }));
        Ok(events)
    }
}

fn shifted(micros: i64, shift: i64) -> Timestamp {
    Timestamp::from_micros(u64::try_from(micros.saturating_add(shift)).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const SPS: [u8; 24] = [
        0x67, 0x42, 0xd0, 0x0b, 0xda, 0x0a, 0x37, 0xe4, 0xc0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04,
        0x00, 0x00, 0x03, 0x00, 0x78, 0x3c, 0x48, 0x9a, 0x80,
    ];
    const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
    const KEYFRAME_SLICE: [u8; 3] = [0x65, 0x88, 0x84];
    const DELTA_SLICE: [u8; 3] = [0x41, 0x9a, 0x02];
    /// AAC-LC, 48 kHz, stereo.
    const AUDIO_SPECIFIC_CONFIG: [u8; 2] = [0x11, 0x90];

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|nal| [0, 0, 0, 1].into_iter().chain(nal.iter().copied()))
            .collect()
    }

    fn video_sample(events: &[MediaEvent]) -> &mediapack::VideoSample {
        events
            .iter()
            .find_map(|event| match event {
                MediaEvent::Video(sample) => Some(sample),
                _ => None,
            })
            .expect("a video sample")
    }

    #[test]
    fn converts_ticks_of_a_fractional_frame_rate() {
        // Act / Assert: 30000/1001 fps
        assert_eq!(ticks_to_micros(30, 1001, 30_000), Some(1_001_000));
        assert_eq!(ticks_to_micros(-2, 1001, 30_000), Some(-66_734));
        assert_eq!(ticks_to_micros(1, 1, 0), None);
    }

    #[test]
    fn takes_the_delay_of_a_frame_stamped_on_the_same_clock_within_the_last_hour() {
        // Act / Assert
        assert_eq!(obs_delay_micros(5_000_000, 4_200_000), Some(800_000));
        assert_eq!(obs_delay_micros(5_000_000, 5_000_000), Some(0));
        assert_eq!(obs_delay_micros(5_000_000, 5_000_001), None);
        assert_eq!(obs_delay_micros(MAX_OBS_DELAY_MICROS + 1, 0), None);
        assert_eq!(obs_delay_micros(i64::MIN, 1), None);
    }

    #[test]
    fn puts_presentation_time_zero_before_the_frame_by_its_presentation_time() {
        // Arrange: a frame shown at 66 ms that OBS composed 800 ms ago
        let wall_now = Timestamp::from_micros(1_000_000_000);

        // Act
        let origin = capture_origin(wall_now, 800_000, Timestamp::from_micros(66_000));

        // Assert
        assert_eq!(origin, Timestamp::from_micros(999_134_000));
    }

    #[test]
    fn completes_a_keyframe_with_the_parameter_sets_of_the_extra_data() {
        // Arrange
        let mut input = MediaInput::default();
        let extra_data = annexb(&[&SPS, &PPS]);
        let keyframe = annexb(&[&KEYFRAME_SLICE]);

        // Act
        let events = input.video(&extra_data, &keyframe, 0, -66_000).unwrap();

        // Assert: the config comes first; the DTS starts the clock and the PTS keeps its lead
        assert!(matches!(events[0], MediaEvent::VideoConfig(_)));
        let sample = video_sample(&events);
        assert!(sample.is_keyframe);
        assert_eq!(sample.data, annexb(&[&SPS, &PPS, &KEYFRAME_SLICE]));
        assert_eq!(sample.dts, Timestamp::ZERO);
        assert_eq!(sample.pts, Timestamp::from_micros(66_000));
    }

    #[test]
    fn puts_audio_on_the_video_clock_and_sends_its_config_once() {
        // Arrange
        let mut input = MediaInput::default();
        let extra_data = annexb(&[&SPS, &PPS]);
        input
            .video(&extra_data, &annexb(&[&KEYFRAME_SLICE]), 0, -66_000)
            .unwrap();

        // Act
        let first = input.audio(&AUDIO_SPECIFIC_CONFIG, &[1, 2, 3], 0).unwrap();
        let second = input
            .audio(&AUDIO_SPECIFIC_CONFIG, &[4, 5, 6], 21_333)
            .unwrap();

        // Assert
        assert!(matches!(
            &first[0],
            MediaEvent::AudioConfig(AudioConfig::Aac(config)) if config.sample_rate == 48_000
        ));
        assert!(matches!(
            &first[1],
            MediaEvent::Audio(sample) if sample.pts == Timestamp::from_micros(66_000)
        ));
        assert_eq!(second.len(), 1);
        assert!(matches!(
            &second[0],
            MediaEvent::Audio(sample) if sample.pts == Timestamp::from_micros(87_333)
        ));
    }

    #[test]
    fn leaves_out_audio_that_comes_before_the_first_video_packet() {
        // Arrange
        let mut input = MediaInput::default();

        // Act
        let events = input.audio(&AUDIO_SPECIFIC_CONFIG, &[1, 2, 3], 0).unwrap();

        // Assert
        assert!(events.is_empty());
    }

    proptest! {
        #[test]
        fn a_frame_is_captured_its_delay_before_now_whatever_its_presentation_time(
            wall_now in prop_oneof![0u64..10_000_000, 1_700_000_000_000_000u64..1_900_000_000_000_000],
            obs_now in prop_oneof![0i64..10_000_000, 0i64..100_000_000_000_000],
            delay in prop_oneof![0i64..3, 0i64..=MAX_OBS_DELAY_MICROS],
            presentation in prop_oneof![0u64..200_000, 0u64..86_400_000_000],
        ) {
            // Act
            let delay_micros = obs_delay_micros(obs_now, obs_now - delay).unwrap();
            let origin = capture_origin(
                Timestamp::from_micros(wall_now),
                delay_micros,
                Timestamp::from_micros(presentation),
            );

            // Assert: the origin is `presentation` before the capture unless it stops at zero
            let captured = wall_now.saturating_sub(delay as u64);
            prop_assert!(origin.micros() <= captured);
            prop_assert_eq!(origin.micros(), captured.saturating_sub(presentation));
        }

        #[test]
        fn a_stamp_outside_the_last_hour_gives_no_delay(
            obs_now in -1_000_000i64..100_000_000_000_000,
            outside in prop_oneof![
                1i64..1_000_000_000_000,
                (MAX_OBS_DELAY_MICROS + 1..1_000_000_000_000_000).prop_map(|delay| -delay),
            ],
        ) {
            // Act / Assert: `outside` puts the stamp in the future or more than an hour back
            prop_assert_eq!(obs_delay_micros(obs_now, obs_now + outside), None);
        }

        #[test]
        fn later_ticks_never_map_to_earlier_microseconds(
            ticks in prop_oneof![-1_000i64..1_000, -1_000_000i64..1_000_000_000],
            step in prop_oneof![0i64..4, 0i64..1_000_000],
            (timebase_num, timebase_den) in prop_oneof![
                Just((1, 30)),
                Just((1001, 30_000)),
                Just((1001, 60_000)),
                Just((1, 48_000)),
                (1..=1001i32, 1..=90_000i32),
            ],
        ) {
            // Act
            let earlier = ticks_to_micros(ticks, timebase_num, timebase_den).unwrap();
            let later = ticks_to_micros(ticks + step, timebase_num, timebase_den).unwrap();

            // Assert
            prop_assert!(earlier <= later);
        }

        #[test]
        fn video_keeps_its_spacing_and_starts_decoding_at_zero(
            first_dts in -1_000_000i64..1_000_000_000_000,
            reorder_delay in 0i64..200_000,
            frame_durations in prop::collection::vec(1i64..100_000, 1..40),
        ) {
            // Arrange: every frame is shown `reorder_delay` after it is decoded
            let mut input = MediaInput::default();
            let extra_data = annexb(&[&SPS, &PPS]);
            let mut dts = first_dts;

            for (index, duration) in frame_durations.iter().enumerate() {
                let slice: &[u8] = if index == 0 { &KEYFRAME_SLICE } else { &DELTA_SLICE };

                // Act
                let events = input
                    .video(&extra_data, &annexb(&[slice]), dts + reorder_delay, dts)
                    .unwrap();

                // Assert
                let sample = video_sample(&events);
                prop_assert_eq!(sample.dts.micros(), (dts - first_dts) as u64);
                prop_assert_eq!(sample.pts.micros(), (dts - first_dts + reorder_delay) as u64);
                dts += duration;
            }
        }
    }
}
