use mediapack::{AudioSample, MediaEvent, Timestamp, VideoSample};

/// A track lowers its shift by at most 1/`SLEW_DIVISOR` of the decode time it
/// advanced since its previous sample, so its presentation times keep
/// advancing at no less than 7/8 of the media rate while it catches up: a
/// 30 fps frame stays at least 29 ms after the previous one, and 200 ms of
/// lateness is absorbed within 1.6 s.
const SLEW_DIVISOR: i64 = 8;

/// Shifts the presentation times of one RTSP session so that the capture time
/// the broadcast derives from them (wall clock at the first sample plus
/// presentation time) never lies after the sample's arrival.
///
/// The capture origin comes from the first sample, so a late first sample
/// would stamp every later sample into the future and show viewers a negative
/// delay. The target shift follows the earliest arrival seen on either track
/// and never rises. Each track lowers its own shift towards the target by at
/// most a fixed fraction of its own media time, so samples of the other track
/// in between never make it stall or run backwards, and once both reach the
/// target they share it. A track's first sample starts at the shift of the
/// track already running, keeping audio and video aligned.
#[derive(Default)]
pub(crate) struct ArrivalClamp {
    origin_us: Option<i64>,
    target_shift_us: i64,
    video: TrackShift,
    audio: TrackShift,
}

#[derive(Default)]
struct TrackShift {
    shift_us: i64,
    last_decode_us: Option<i64>,
}

#[derive(Clone, Copy)]
enum Track {
    Video,
    Audio,
}

impl ArrivalClamp {
    pub(crate) fn apply(&mut self, event: &mut MediaEvent, arrival_us: i64) {
        match event {
            MediaEvent::Video(VideoSample { pts, dts, .. }) => {
                let shift_us = self.follow(Track::Video, *pts, *dts, arrival_us);
                *pts = shifted(*pts, shift_us);
                *dts = shifted(*dts, shift_us);
            }
            MediaEvent::Audio(AudioSample { pts, .. }) => {
                let shift_us = self.follow(Track::Audio, *pts, *pts, arrival_us);
                *pts = shifted(*pts, shift_us);
            }
            _ => {}
        }
    }

    /// Measures the track's pace on the decode time, which keeps increasing when
    /// B-frames reorder the presentation times.
    fn follow(&mut self, track: Track, pts: Timestamp, dts: Timestamp, arrival_us: i64) -> i64 {
        let pts_us = pts.micros() as i64;
        let origin_us = *self.origin_us.get_or_insert(arrival_us - pts_us);
        let lead_us = origin_us + pts_us + self.target_shift_us - arrival_us;
        if lead_us > 0 {
            self.target_shift_us -= lead_us;
        }
        let target_shift_us = self.target_shift_us;
        let (this, other) = match track {
            Track::Video => (&mut self.video, &self.audio),
            Track::Audio => (&mut self.audio, &self.video),
        };
        let decode_us = dts.micros() as i64;
        this.shift_us = match this.last_decode_us {
            Some(last_decode_us) => {
                let step_us = (decode_us - last_decode_us).max(0) / SLEW_DIVISOR;
                target_shift_us.max(this.shift_us - step_us)
            }
            None if other.last_decode_us.is_some() => other.shift_us,
            None => target_shift_us,
        };
        this.last_decode_us = Some(decode_us);
        this.shift_us
    }
}

fn shifted(time: Timestamp, shift_us: i64) -> Timestamp {
    Timestamp::from_micros((time.micros() as i64 + shift_us).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    const START_US: i64 = 1_000_000_000;

    fn video(pts_us: u64) -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: Bytes::new(),
            is_keyframe: false,
            pts: Timestamp::from_micros(pts_us),
            dts: Timestamp::from_micros(pts_us),
        })
    }

    fn audio(pts_us: u64) -> MediaEvent {
        MediaEvent::Audio(AudioSample {
            data: Bytes::new(),
            pts: Timestamp::from_micros(pts_us),
            audio_level: None,
        })
    }

    fn pts_of(event: &MediaEvent) -> u64 {
        match event {
            MediaEvent::Video(sample) => sample.pts.micros(),
            MediaEvent::Audio(sample) => sample.pts.micros(),
            _ => unreachable!("the tests only build video and audio samples"),
        }
    }

    fn clamp_all(clamp: &mut ArrivalClamp, samples: &[(u64, i64)]) -> Vec<u64> {
        samples
            .iter()
            .map(|&(pts_us, arrival_us)| {
                let mut event = video(pts_us);
                clamp.apply(&mut event, START_US + arrival_us);
                pts_of(&event)
            })
            .collect()
    }

    #[test]
    fn samples_arriving_as_steadily_as_the_first_keep_their_presentation_times() {
        // Arrange
        let mut clamp = ArrivalClamp::default();

        // Act
        let pts = clamp_all(&mut clamp, &[(0, 0), (66_000, 66_000), (132_000, 140_000)]);

        // Assert
        assert_eq!(pts, [0, 66_000, 132_000]);
    }

    #[test]
    fn samples_arriving_earlier_than_the_first_move_back_gradually() {
        // Arrange: the first sample arrived 200 ms late
        let mut clamp = ArrivalClamp::default();

        // Act
        let pts = clamp_all(
            &mut clamp,
            &[
                (0, 200_000),
                (66_000, 66_000),
                (132_000, 132_000),
                (198_000, 198_000),
            ],
        );

        // Assert: each 66 ms frame moves back by an eighth of it
        assert_eq!(pts, [0, 57_750, 115_500, 173_250]);
    }

    #[test]
    fn shift_settles_where_the_samples_no_longer_arrive_before_their_capture_time() {
        // Arrange
        let mut clamp = ArrivalClamp::default();
        let samples: Vec<(u64, i64)> = std::iter::once((0, 200_000))
            .chain((1..=60).map(|frame| (frame * 66_000, frame as i64 * 66_000)))
            .collect();

        // Act
        let pts = clamp_all(&mut clamp, &samples);

        // Assert
        assert_eq!(*pts.last().unwrap(), 60 * 66_000 - 200_000);
        assert!(pts.windows(2).all(|pair| pair[0] < pair[1]));
    }

    /// Clamps one second of 30 fps video and of audio in `audio_packet_us`
    /// packets, interleaved in presentation order and arriving on time except
    /// for the first sample, 200 ms late; returns the video and audio
    /// presentation times.
    fn clamp_late_session(clamp: &mut ArrivalClamp, audio_packet_us: u64) -> (Vec<u64>, Vec<u64>) {
        let mut events: Vec<MediaEvent> = (0..30)
            .map(|frame| video(frame * 33_333))
            .chain((0..1_000_000 / audio_packet_us).map(|packet| audio(packet * audio_packet_us)))
            .collect();
        events.sort_by_key(pts_of);
        let (mut video_pts, mut audio_pts) = (Vec::new(), Vec::new());
        for (index, mut event) in events.into_iter().enumerate() {
            let late_us = if index == 0 { 200_000 } else { 0 };
            let arrival_us = START_US + pts_of(&event) as i64 + late_us;
            clamp.apply(&mut event, arrival_us);
            match event {
                MediaEvent::Video(sample) => video_pts.push(sample.pts.micros()),
                MediaEvent::Audio(sample) => audio_pts.push(sample.pts.micros()),
                _ => unreachable!("the session holds only video and audio samples"),
            }
        }
        (video_pts, audio_pts)
    }

    fn strictly_increasing(pts: &[u64]) -> bool {
        pts.windows(2).all(|pair| pair[0] < pair[1])
    }

    #[test]
    fn video_keeps_moving_forward_between_5_ms_audio_packets() {
        // Arrange
        let mut clamp = ArrivalClamp::default();

        // Act
        let (video_pts, audio_pts) = clamp_late_session(&mut clamp, 5_000);

        // Assert
        assert!(strictly_increasing(&video_pts), "{video_pts:?}");
        assert!(strictly_increasing(&audio_pts), "{audio_pts:?}");
    }

    #[test]
    fn audio_in_10_ms_packets_keeps_moving_forward_between_video_frames() {
        // Arrange
        let mut clamp = ArrivalClamp::default();

        // Act
        let (video_pts, audio_pts) = clamp_late_session(&mut clamp, 10_000);

        // Assert
        assert!(strictly_increasing(&video_pts), "{video_pts:?}");
        assert!(strictly_increasing(&audio_pts), "{audio_pts:?}");
    }

    #[test]
    fn video_and_audio_settle_on_the_same_shift() {
        // Arrange
        let mut clamp = ArrivalClamp::default();
        clamp_late_session(&mut clamp, 20_000);

        // Act
        let mut video_frame = video(2_000_000);
        clamp.apply(&mut video_frame, START_US + 2_000_000);
        let mut audio_packet = audio(2_000_000);
        clamp.apply(&mut audio_packet, START_US + 2_000_000);

        // Assert
        assert_eq!(pts_of(&video_frame), 1_800_000);
        assert_eq!(pts_of(&audio_packet), 1_800_000);
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;

        #[derive(Debug, Clone, Copy, PartialEq)]
        enum Kind {
            Video,
            Audio,
        }

        #[derive(Debug, Clone)]
        struct Sample {
            kind: Kind,
            pts_us: u64,
            arrival_us: i64,
        }

        fn video_duration_us() -> impl Strategy<Value = u64> {
            prop_oneof![
                3 => Just(33_333u64),
                3 => Just(40_000u64),
                1 => 1_000u64..=40_000,
            ]
        }

        fn audio_duration_us() -> impl Strategy<Value = u64> {
            prop_oneof![
                2 => Just(5_000u64),
                2 => Just(10_000u64),
                2 => Just(20_000u64),
                1 => 1_000u64..=40_000,
            ]
        }

        fn timeline(kind: Kind, duration_us: u64, count: usize) -> Vec<(Kind, u64)> {
            (0..count as u64)
                .map(|index| (kind, index * duration_us))
                .collect()
        }

        /// One RTSP session: a video and an audio track interleaved in
        /// presentation order, each sample arriving after a random network
        /// jitter, the first one late by up to half a second.
        fn session() -> impl Strategy<Value = Vec<Sample>> {
            (
                video_duration_us(),
                audio_duration_us(),
                1usize..=60,
                0u64..=40_000,
                prop_oneof![Just(0i64), 0i64..=500_000],
            )
                .prop_flat_map(
                    |(
                        video_duration_us,
                        audio_duration_us,
                        frames,
                        audio_start_us,
                        first_late_us,
                    )| {
                        let span_us = frames as u64 * video_duration_us;
                        let audio_count =
                            (span_us.saturating_sub(audio_start_us) / audio_duration_us) as usize;
                        let mut events = timeline(Kind::Video, video_duration_us, frames);
                        events.extend(
                            timeline(Kind::Audio, audio_duration_us, audio_count)
                                .into_iter()
                                .map(|(kind, pts_us)| (kind, pts_us + audio_start_us)),
                        );
                        events.sort_by_key(|&(_, pts_us)| pts_us);
                        let len = events.len();
                        (
                            Just(events),
                            proptest::collection::vec(prop_oneof![Just(0i64), 0i64..=30_000], len),
                            Just(first_late_us),
                        )
                    },
                )
                .prop_map(|(events, jitters_us, first_late_us)| {
                    let mut last_arrival_us = i64::MIN;
                    events
                        .into_iter()
                        .zip(jitters_us)
                        .enumerate()
                        .map(|(index, ((kind, pts_us), jitter_us))| {
                            let late_us = if index == 0 { first_late_us } else { 0 };
                            // The wall clock is read as each event is handled, so
                            // it never runs backwards.
                            last_arrival_us =
                                last_arrival_us.max(START_US + pts_us as i64 + jitter_us + late_us);
                            Sample {
                                kind,
                                pts_us,
                                arrival_us: last_arrival_us,
                            }
                        })
                        .collect()
                })
        }

        fn event(sample: &Sample) -> MediaEvent {
            match sample.kind {
                Kind::Video => video(sample.pts_us),
                Kind::Audio => audio(sample.pts_us),
            }
        }

        proptest! {
            #[test]
            fn each_track_keeps_increasing_and_settles_no_later_than_arrival(
                samples in session(),
            ) {
                // Arrange
                let mut clamp = ArrivalClamp::default();
                let origin_us = samples[0].arrival_us - samples[0].pts_us as i64;
                let mut last_pts_us = [None::<u64>; 2];

                for sample in &samples {
                    // Act
                    let mut event = event(sample);
                    clamp.apply(&mut event, sample.arrival_us);
                    let pts_us = pts_of(&event);

                    // Assert: a track's presentation times strictly increase
                    let last = &mut last_pts_us[sample.kind as usize];
                    if let Some(last) = *last {
                        prop_assert!(
                            last < pts_us,
                            "{:?} went from {last} to {pts_us} at input {}",
                            sample.kind,
                            sample.pts_us
                        );
                    }
                    *last = Some(pts_us);
                    // Assert: once the track's shift has settled, no capture time lies after arrival
                    let track = match sample.kind {
                        Kind::Video => &clamp.video,
                        Kind::Audio => &clamp.audio,
                    };
                    if track.shift_us == clamp.target_shift_us {
                        prop_assert!(origin_us + pts_us as i64 <= sample.arrival_us);
                    }
                }
            }
        }
    }
}
