use mediapack::{AudioSample, MediaEvent, Timestamp, VideoSample};

/// Largest step one sample may move the presentation times back, so a
/// correction never makes a track's timestamps run backwards (camera frames
/// and audio packets are tens of milliseconds apart).
const MAX_SLEW_US: i64 = 5_000;

/// Shifts the presentation times of one RTSP session so that the capture time
/// the broadcast derives from them (wall clock at the first sample plus
/// presentation time) never lies after the sample's arrival.
///
/// The capture origin comes from the first sample, so a late first sample
/// would stamp every later sample into the future and show viewers a negative
/// delay. The shift follows the earliest arrival seen and moves gradually.
#[derive(Default)]
pub struct ArrivalClamp {
    origin_us: Option<i64>,
    shift_us: i64,
    target_shift_us: i64,
}

impl ArrivalClamp {
    pub fn apply(&mut self, event: &mut MediaEvent, arrival_us: i64) {
        match event {
            MediaEvent::Video(VideoSample { pts, dts, .. }) => {
                self.follow(*pts, arrival_us);
                *pts = self.shifted(*pts);
                *dts = self.shifted(*dts);
            }
            MediaEvent::Audio(AudioSample { pts, .. }) => {
                self.follow(*pts, arrival_us);
                *pts = self.shifted(*pts);
            }
            _ => {}
        }
    }

    fn follow(&mut self, pts: Timestamp, arrival_us: i64) {
        let pts_us = pts.micros() as i64;
        let origin_us = *self.origin_us.get_or_insert(arrival_us - pts_us);
        let lead_us = origin_us + pts_us + self.target_shift_us - arrival_us;
        if lead_us > 0 {
            self.target_shift_us -= lead_us;
        }
        self.shift_us = self.target_shift_us.max(self.shift_us - MAX_SLEW_US);
    }

    fn shifted(&self, time: Timestamp) -> Timestamp {
        Timestamp::from_micros((time.micros() as i64 + self.shift_us).max(0) as u64)
    }
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

        // Assert
        assert_eq!(pts, [0, 61_000, 122_000, 183_000]);
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
}
