use gstreamer as gst;

use super::loudness::{Loudness, WINDOW};

/// About -40 dBFS: speech into a browser microphone is well above it, the
/// background left by its noise suppression well below.
const SPEECH_RMS: f32 = 0.01;
/// A participant takes over only after this many windows of speech, so a
/// short reply or a cough does not move the picture.
const MIN_TURN_WINDOWS: usize = 10;
/// A silence this long ends a turn.
const PAUSE_WINDOWS: usize = 15;

/// Start times on the composition's timeline, each with the input index of
/// the active speaker from then on. Computed over the whole recording before
/// compositing, so a switch starts where the new speaker starts talking, and
/// the first speaker holds the stage from time 0.
#[derive(Default)]
pub(super) struct Speakers(Vec<(gst::ClockTime, usize)>);

struct Turn {
    speaker: usize,
    start: usize,
    last: usize,
    voiced: usize,
}

impl Speakers {
    /// `loudness` is indexed by input; an input without audio never speaks.
    pub(super) fn detect(loudness: &[Option<&Loudness>]) -> Self {
        let windows = loudness
            .iter()
            .flatten()
            .map(|loudness| loudness.rms.len())
            .max()
            .unwrap_or(0);
        let mut speakers = Self::default();
        let mut turn: Option<Turn> = None;
        for window in 0..windows {
            let loudest = loudest(loudness, window);
            match (loudest, &mut turn) {
                (Some(speaker), Some(current)) if current.speaker == speaker => {
                    current.last = window;
                    current.voiced += 1;
                }
                (None, Some(current)) if window - current.last < PAUSE_WINDOWS => {}
                (None, None) => {}
                _ => {
                    speakers.close(turn.take());
                    turn = loudest.map(|speaker| Turn {
                        speaker,
                        start: window,
                        last: window,
                        voiced: 1,
                    });
                }
            }
        }
        speakers.close(turn);
        speakers
    }

    fn close(&mut self, turn: Option<Turn>) {
        let Some(turn) = turn.filter(|turn| turn.voiced >= MIN_TURN_WINDOWS) else {
            return;
        };
        match self.0.last() {
            Some(&(_, speaker)) if speaker == turn.speaker => {}
            Some(_) => self.0.push((WINDOW * turn.start as u64, turn.speaker)),
            None => self.0.push((gst::ClockTime::ZERO, turn.speaker)),
        }
    }

    /// `None` when nobody ever speaks.
    pub(super) fn at(&self, time: gst::ClockTime) -> Option<usize> {
        let index = self.0.partition_point(|&(start, _)| start <= time);
        index.checked_sub(1).map(|index| self.0[index].1)
    }
}

/// The input that is loudest in `window` and loud enough to be speech.
fn loudest(loudness: &[Option<&Loudness>], window: usize) -> Option<usize> {
    loudness
        .iter()
        .enumerate()
        .filter_map(|(input, loudness)| Some((input, *(*loudness)?.rms.get(window)?)))
        .filter(|(_, rms)| *rms >= SPEECH_RMS)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(input, _)| input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ms;

    const LOUD: f32 = 0.1;
    const QUIET: f32 = 0.05;

    /// One window per character: `-` silent, `q` quiet speech, `L` loud speech.
    fn loudness(pattern: &str) -> Loudness {
        Loudness {
            rms: pattern
                .chars()
                .map(|level| match level {
                    'L' => LOUD,
                    'q' => QUIET,
                    _ => 0.0,
                })
                .collect(),
        }
    }

    #[test]
    fn the_first_speaker_holds_the_stage_from_the_start() {
        // Arrange: input 1 starts talking at 0.5 s
        let a = loudness("--------------------");
        let b = loudness("-----LLLLLLLLLLLLLLL");

        // Act
        let speakers = Speakers::detect(&[Some(&a), Some(&b)]);

        // Assert
        assert_eq!(speakers.0, [(ms(0), 1)]);
        assert_eq!(speakers.at(ms(0)), Some(1));
    }

    #[test]
    fn a_new_speaker_takes_over_where_their_turn_starts() {
        // Arrange: a talks for 1.5 s, b answers from 2 s on
        let a = loudness("LLLLLLLLLLLLLLL-----------------------");
        let b = loudness("--------------------LLLLLLLLLLLLLLLLLL");

        // Act
        let speakers = Speakers::detect(&[Some(&a), Some(&b)]);

        // Assert
        assert_eq!(speakers.0, [(ms(0), 0), (ms(2_000), 1)]);
        assert_eq!(speakers.at(ms(1_999)), Some(0));
        assert_eq!(speakers.at(ms(2_000)), Some(1));
    }

    #[test]
    fn a_short_interjection_does_not_take_over() {
        // Arrange: b says something for 0.5 s while a talks
        let a = loudness("LLLLLLLLLLLL-----LLLLLLLLLLLL");
        let b = loudness("------------LLLLL------------");

        // Act
        let speakers = Speakers::detect(&[Some(&a), Some(&b)]);

        // Assert
        assert_eq!(speakers.0, [(ms(0), 0)]);
    }

    #[test]
    fn short_pauses_stay_within_one_turn() {
        // Arrange: b speaks in bursts of 0.4 s, 0.5 s apart, after a's turn
        let a = loudness("LLLLLLLLLLLL---------------------------");
        let b = loudness("------------LLLL-----LLLL-----LLLL-----");

        // Act
        let speakers = Speakers::detect(&[Some(&a), Some(&b)]);

        // Assert
        assert_eq!(speakers.0, [(ms(0), 0), (ms(1_200), 1)]);
    }

    #[test]
    fn the_louder_of_two_simultaneous_speakers_wins() {
        // Arrange
        let a = loudness("qqqqqqqqqqqq");
        let b = loudness("LLLLLLLLLLLL");

        // Act
        let speakers = Speakers::detect(&[Some(&a), None, Some(&b)]);

        // Assert
        assert_eq!(speakers.0, [(ms(0), 2)]);
    }

    #[test]
    fn nobody_speaks_in_silence() {
        // Arrange
        let a = loudness("------------");

        // Act
        let speakers = Speakers::detect(&[Some(&a), None]);

        // Assert
        assert_eq!(speakers.at(ms(500)), None);
    }
}
