use std::{collections::VecDeque, time::Duration};

use tokio::{sync::mpsc, task::JoinHandle, time::Instant};

use crate::{
    broadcast_translation::sleep_until,
    opus_codec::{GEMINI_INPUT_SAMPLE_RATE, GEMINI_OUTPUT_SAMPLE_RATE},
    translation_engine::TranslationOutput,
};

const INPUT_SAMPLE_RATE: usize = GEMINI_INPUT_SAMPLE_RATE as usize;
const OUTPUT_SAMPLE_RATE: usize = GEMINI_OUTPUT_SAMPLE_RATE as usize;

pub(crate) struct EchoTranslation {
    join_handle: JoinHandle<()>,
}

impl EchoTranslation {
    pub(crate) fn run(
        delay: Duration,
        mut pcm_receiver: mpsc::Receiver<Vec<i16>>,
        translated_sender: mpsc::Sender<TranslationOutput>,
    ) -> Self {
        let join_handle = tokio::spawn(async move {
            let mut delayed: VecDeque<(Instant, Vec<i16>)> = VecDeque::new();
            loop {
                let next_due = delayed.front().map(|(due, _)| *due);
                tokio::select! {
                    samples = pcm_receiver.recv() => match samples {
                        Some(samples) => delayed.push_back((Instant::now() + delay, upsample(&samples))),
                        None => return,
                    },
                    _ = sleep_until(next_due) => {
                        if let Some((_, samples)) = delayed.pop_front()
                            && translated_sender
                                .send(TranslationOutput::Audio(samples))
                                .await
                                .is_err()
                        {
                            return;
                        }
                    }
                }
            }
        });
        Self { join_handle }
    }
}

impl Drop for EchoTranslation {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

fn upsample(samples: &[i16]) -> Vec<i16> {
    let output_len = samples.len() * OUTPUT_SAMPLE_RATE / INPUT_SAMPLE_RATE;
    (0..output_len)
        .map(|index| samples[index * INPUT_SAMPLE_RATE / OUTPUT_SAMPLE_RATE])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsampling_keeps_the_duration() {
        // Act
        let output = upsample(&vec![7; 320]);

        // Assert
        assert_eq!(output.len(), 480);
        assert!(output.iter().all(|&sample| sample == 7));
    }

    #[tokio::test(start_paused = true)]
    async fn input_comes_back_after_the_delay() {
        // Arrange
        let (pcm_sender, pcm_receiver) = mpsc::channel(4);
        let (translated_sender, mut translated_receiver) = mpsc::channel(4);
        let _echo = EchoTranslation::run(Duration::from_secs(3), pcm_receiver, translated_sender);
        let started = Instant::now();

        // Act
        pcm_sender
            .send(vec![1; 320])
            .await
            .expect("echo is running");
        let Some(TranslationOutput::Audio(echoed)) = translated_receiver.recv().await else {
            panic!("echo returns audio");
        };

        // Assert
        assert_eq!(echoed.len(), 480);
        assert!(started.elapsed() >= Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn back_to_back_inputs_are_each_delayed_once() {
        // Arrange
        let (pcm_sender, pcm_receiver) = mpsc::channel(4);
        let (translated_sender, mut translated_receiver) = mpsc::channel(4);
        let _echo = EchoTranslation::run(Duration::from_secs(3), pcm_receiver, translated_sender);
        let started = Instant::now();

        // Act
        pcm_sender
            .send(vec![1; 320])
            .await
            .expect("echo is running");
        pcm_sender
            .send(vec![2; 320])
            .await
            .expect("echo is running");
        translated_receiver.recv().await.expect("first echoed");
        let Some(TranslationOutput::Audio(second)) = translated_receiver.recv().await else {
            panic!("echo returns audio");
        };

        // Assert
        assert_eq!(second[0], 2);
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
