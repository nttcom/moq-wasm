use anyhow::{Context, Result, anyhow, bail};
use gstreamer::{self as gst, prelude::*};
use gstreamer_app::AppSink;

use super::{EventSource, app_source, feed_audio, source_properties, with_samples};

/// Length of the windows the level is measured over.
pub(super) const WINDOW: gst::ClockTime = gst::ClockTime::from_mseconds(100);
const ANALYSIS_RATE: u64 = 16_000;
const SAMPLES_PER_WINDOW: u64 = ANALYSIS_RATE * WINDOW.mseconds() / 1_000;
const PULL_TIMEOUT: gst::ClockTime = gst::ClockTime::from_mseconds(100);

/// One participant's audio level over time; the compositor picks the active
/// speaker from it.
#[derive(Debug)]
pub struct Loudness {
    /// RMS amplitude, full scale 1.0, of each `WINDOW` from presentation
    /// time 0; a window without audio is 0.
    pub(super) rms: Vec<f32>,
}

impl Loudness {
    /// Decodes an audio source whose timestamps are on the composition's
    /// timeline. Blocks until the whole source is read; fails when it cannot
    /// be decoded. A source without samples is silent.
    pub fn measure(events: EventSource) -> Result<Self> {
        let Some(events) = with_samples(events)? else {
            return Ok(Self { rms: Vec::new() });
        };
        gst::init()?;
        let pipeline = gst::parse::launch(&format!(
            "appsrc name=audio {} ! decodebin3 ! audioconvert ! audioresample ! \
             audio/x-raw,format=F32LE,layout=interleaved,channels=1,rate={ANALYSIS_RATE} ! \
             appsink name=sink sync=false",
            source_properties()
        ))
        .context("build loudness pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("loudness description did not produce a pipeline"))?;
        let source = app_source(&pipeline, "audio")?;
        let sink = pipeline
            .by_name("sink")
            .context("missing appsink")?
            .downcast::<AppSink>()
            .map_err(|_| anyhow!("sink is not an appsink"))?;
        let bus = pipeline.bus().context("loudness pipeline has no bus")?;
        pipeline
            .set_state(gst::State::Playing)
            .context("start loudness pipeline")?;
        let (failure_sender, failure_receiver) = std::sync::mpsc::channel();
        let feeder = std::thread::Builder::new()
            .name("loudness-audio".to_string())
            .spawn(move || {
                if let Err(error) = feed_audio(&source, events) {
                    let _ = failure_sender.send(error);
                }
            })
            .context("spawn loudness feeder")?;
        let measured = pull_levels(&sink, &bus, &failure_receiver);
        // Unblocks a feeder still pushing into a stopped pipeline.
        let _ = pipeline.set_state(gst::State::Null);
        feeder
            .join()
            .map_err(|_| anyhow!("loudness feeder panicked"))?;
        measured
    }
}

/// Also watches the feeder's failure: a failing source ends neither the
/// stream nor the pipeline.
fn pull_levels(
    sink: &AppSink,
    bus: &gst::Bus,
    failure_receiver: &std::sync::mpsc::Receiver<anyhow::Error>,
) -> Result<Loudness> {
    let mut levels = Levels::default();
    loop {
        if let Some(sample) = sink.try_pull_sample(PULL_TIMEOUT) {
            let buffer = sample
                .buffer()
                .context("loudness sample without a buffer")?;
            let pts = buffer.pts().context("decoded audio without a timestamp")?;
            let map = buffer
                .map_readable()
                .map_err(|_| anyhow!("cannot read decoded audio"))?;
            let samples = map
                .as_slice()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes));
            levels.add(pts, samples);
            continue;
        }
        if let Ok(error) = failure_receiver.try_recv() {
            return Err(error.context("feed loudness audio"));
        }
        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error])
            && let gst::MessageView::Error(error) = message.view()
        {
            bail!(
                "loudness pipeline error: {} ({:?})",
                error.error(),
                error.debug()
            );
        }
        if sink.is_eos() {
            return Ok(levels.finish());
        }
    }
}

#[derive(Default)]
struct Levels {
    squares: Vec<f64>,
    counts: Vec<u64>,
}

impl Levels {
    fn add(&mut self, pts: gst::ClockTime, samples: impl Iterator<Item = f32>) {
        let first = pts.nseconds() * ANALYSIS_RATE / gst::ClockTime::SECOND.nseconds();
        for (offset, sample) in samples.enumerate() {
            let window = ((first + offset as u64) / SAMPLES_PER_WINDOW) as usize;
            if window >= self.squares.len() {
                self.squares.resize(window + 1, 0.0);
                self.counts.resize(window + 1, 0);
            }
            self.squares[window] += f64::from(sample) * f64::from(sample);
            self.counts[window] += 1;
        }
    }

    fn finish(self) -> Loudness {
        let rms = self
            .squares
            .into_iter()
            .zip(self.counts)
            .map(|(squares, count)| match count {
                0 => 0.0,
                count => (squares / count as f64).sqrt() as f32,
            })
            .collect();
        Loudness { rms }
    }
}

#[cfg(test)]
mod tests {
    use mediapack::{MediaEvent, Timestamp};

    use super::*;
    use crate::test_support::{audio_events, fixture_events, shifted, source};

    #[test]
    fn measures_each_window_on_the_composition_timeline() {
        // Arrange: the same tone, once 1 s later on the composition's timeline
        let (on_time, later) = (
            source(audio_events()).unwrap(),
            source(shifted(audio_events(), Timestamp::from_millis(1_000))).unwrap(),
        );

        // Act
        let on_time = Loudness::measure(on_time).unwrap();
        let later = Loudness::measure(later).unwrap();

        // Assert: ten more silent windows before the tone, which is well above silence
        let start = on_time.rms.iter().position(|rms| *rms > 0.0).unwrap();
        assert_eq!(
            later.rms.iter().position(|rms| *rms > 0.0),
            Some(start + 10)
        );
        assert!(on_time.rms[start + 1..].iter().all(|rms| *rms > 0.05));
    }

    #[test]
    fn a_source_without_samples_is_silent() {
        // Arrange: a recorded audio track that never received a sample
        let events: Vec<MediaEvent> = fixture_events()
            .into_iter()
            .filter(|event| matches!(event, MediaEvent::AudioConfig(_)))
            .collect();

        // Act
        let loudness = Loudness::measure(Box::new(events.into_iter().map(Ok))).unwrap();

        // Assert
        assert!(loudness.rms.is_empty());
    }

    #[test]
    fn rejects_a_source_that_fails() {
        // Arrange
        let events: EventSource = Box::new(std::iter::once(Err(anyhow!("broken file"))));

        // Act / Assert
        assert!(Loudness::measure(events).is_err());
    }

    #[test]
    fn averages_the_samples_of_a_window() {
        // Arrange
        let mut levels = Levels::default();

        // Act: 0.5 amplitude for the first window, nothing in the second, half a window at 1.0 in the third
        levels.add(gst::ClockTime::ZERO, std::iter::repeat_n(0.5, 1_600));
        levels.add(
            gst::ClockTime::from_mseconds(200),
            std::iter::repeat_n(-1.0, 800),
        );
        let loudness = levels.finish();

        // Assert
        assert_eq!(loudness.rms, [0.5, 0.0, 1.0]);
    }
}
