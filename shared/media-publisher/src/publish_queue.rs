use anyhow::{Result, anyhow, bail};
use mediapack::MediaEvent;
use tokio::sync::{
    mpsc::{self, error::TrySendError},
    oneshot,
};

/// About three seconds of 30 fps video with AAC audio: a relay stall shorter
/// than this is absorbed, a longer one drops media rather than holding back
/// the ingest, whose SRT or RTMP sender gives up on a connection that is not
/// drained.
const CAPACITY: usize = 256;
/// Codec configurations are never dropped, because a catalog that misses one
/// leaves viewers unable to decode the media that follows; samples leave this
/// many slots free for them.
const CONFIG_HEADROOM: usize = 8;
const DROP_LOG_INTERVAL: u64 = 500;

/// Hands media to the publish task without ever waiting for the relay. When
/// the queue is full, samples are dropped, and video stays dropped until a
/// keyframe fits so that no viewer receives a group it cannot decode from its
/// start.
pub(crate) struct PublishQueue {
    event_sender: mpsc::Sender<MediaEvent>,
    failure_receiver: oneshot::Receiver<anyhow::Error>,
    awaiting_keyframe: bool,
    dropped: DroppedSamples,
}

pub(crate) struct QueueConsumer {
    pub(crate) event_receiver: mpsc::Receiver<MediaEvent>,
    pub(crate) failure_sender: oneshot::Sender<anyhow::Error>,
}

#[derive(Default)]
struct DroppedSamples {
    video: u64,
    audio: u64,
}

impl DroppedSamples {
    fn total(&self) -> u64 {
        self.video + self.audio
    }
}

impl PublishQueue {
    pub(crate) fn open() -> (Self, QueueConsumer) {
        let (event_sender, event_receiver) = mpsc::channel(CAPACITY);
        let (failure_sender, failure_receiver) = oneshot::channel();
        (
            Self {
                event_sender,
                failure_receiver,
                awaiting_keyframe: false,
                dropped: DroppedSamples::default(),
            },
            QueueConsumer {
                event_receiver,
                failure_sender,
            },
        )
    }

    pub(crate) fn push(&mut self, event: MediaEvent) -> Result<()> {
        let has_room = self.event_sender.capacity() > CONFIG_HEADROOM;
        match &event {
            MediaEvent::Video(sample)
                if !has_room || (self.awaiting_keyframe && !sample.is_keyframe) =>
            {
                self.dropped.video += 1;
                self.record_drop();
                return Ok(());
            }
            MediaEvent::Audio(_) if !has_room => {
                self.dropped.audio += 1;
                self.record_drop();
                return Ok(());
            }
            MediaEvent::Streams(_) if !has_room => return Ok(()),
            _ => {}
        }
        let resumes_video = self.awaiting_keyframe && matches!(&event, MediaEvent::Video(_));
        match self.event_sender.try_send(event) {
            Ok(()) => {
                if resumes_video {
                    self.resume();
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                bail!("publish queue full: the relay is not keeping up")
            }
            Err(TrySendError::Closed(_)) => Err(self.failure()),
        }
    }

    fn record_drop(&mut self) {
        if !self.awaiting_keyframe {
            tracing::warn!(
                capacity = CAPACITY,
                "publish queue full: the relay is not keeping up, dropping media until a keyframe fits"
            );
        }
        self.awaiting_keyframe = true;
        if self.dropped.total().is_multiple_of(DROP_LOG_INTERVAL) {
            tracing::warn!(
                dropped_video = self.dropped.video,
                dropped_audio = self.dropped.audio,
                "still dropping media: the publish queue is full"
            );
        }
    }

    fn resume(&mut self) {
        tracing::warn!(
            dropped_video = self.dropped.video,
            dropped_audio = self.dropped.audio,
            "publishing resumed at a keyframe"
        );
        self.awaiting_keyframe = false;
        self.dropped = DroppedSamples::default();
    }

    fn failure(&mut self) -> anyhow::Error {
        match self.failure_receiver.try_recv() {
            Ok(cause) => cause.context("media publishing stopped"),
            Err(_) => anyhow!("media publishing stopped"),
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use mediapack::{AudioSample, Timestamp, VideoSample, aac::AudioSpecificConfig};

    use super::*;

    const SAMPLE_SLOTS: usize = CAPACITY - CONFIG_HEADROOM;

    fn video(is_keyframe: bool) -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: Bytes::from_static(b"nal"),
            is_keyframe,
            pts: Timestamp::ZERO,
            dts: Timestamp::ZERO,
        })
    }

    fn audio() -> MediaEvent {
        MediaEvent::Audio(AudioSample {
            data: Bytes::from_static(b"aac"),
            pts: Timestamp::ZERO,
        })
    }

    fn audio_config() -> MediaEvent {
        MediaEvent::AudioConfig(AudioSpecificConfig::new(2, 48_000, 1))
    }

    fn stalled_queue() -> (PublishQueue, QueueConsumer) {
        let (mut queue, consumer) = PublishQueue::open();
        for _ in 0..SAMPLE_SLOTS {
            queue.push(video(false)).unwrap();
        }
        (queue, consumer)
    }

    fn free_slots(consumer: &mut QueueConsumer, count: usize) {
        for _ in 0..count {
            consumer.event_receiver.try_recv().unwrap();
        }
    }

    fn queued(consumer: &mut QueueConsumer) -> Vec<MediaEvent> {
        let mut events = Vec::new();
        while let Ok(event) = consumer.event_receiver.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn drops_samples_instead_of_waiting_while_the_queue_is_full() {
        // Arrange
        let (mut queue, mut consumer) = stalled_queue();

        // Act
        let delta = queue.push(video(false));
        let keyframe = queue.push(video(true));
        let audio = queue.push(audio());

        // Assert
        assert!(delta.is_ok() && keyframe.is_ok() && audio.is_ok());
        assert_eq!(queued(&mut consumer).len(), SAMPLE_SLOTS);
    }

    #[test]
    fn keeps_dropping_video_until_a_keyframe_after_a_drop() {
        // Arrange
        let (mut queue, mut consumer) = stalled_queue();
        queue.push(video(false)).unwrap();
        free_slots(&mut consumer, 4);

        // Act
        queue.push(video(false)).unwrap();
        queue.push(video(true)).unwrap();
        queue.push(video(false)).unwrap();

        // Assert
        let events = queued(&mut consumer);
        assert_eq!(events.len(), SAMPLE_SLOTS - 4 + 2);
        assert_eq!(events[events.len() - 2..], [video(true), video(false)]);
    }

    #[test]
    fn lets_audio_through_as_soon_as_there_is_room_while_video_waits_for_a_keyframe() {
        // Arrange
        let (mut queue, mut consumer) = stalled_queue();
        queue.push(audio()).unwrap();
        free_slots(&mut consumer, 4);

        // Act
        queue.push(audio()).unwrap();
        queue.push(video(false)).unwrap();

        // Assert
        let events = queued(&mut consumer);
        assert_eq!(events.len(), SAMPLE_SLOTS - 4 + 1);
        assert_eq!(events[events.len() - 1], audio());
    }

    #[test]
    fn reserves_headroom_for_codec_configurations() {
        // Arrange
        let (mut queue, mut consumer) = stalled_queue();

        // Act
        queue.push(audio_config()).unwrap();
        queue.push(audio()).unwrap();

        // Assert
        let events = queued(&mut consumer);
        assert_eq!(events.len(), SAMPLE_SLOTS + 1);
        assert_eq!(events[events.len() - 1], audio_config());
    }

    #[test]
    fn fails_when_even_the_configuration_headroom_is_exhausted() {
        // Arrange
        let (mut queue, _consumer) = stalled_queue();
        for _ in 0..CONFIG_HEADROOM {
            queue.push(audio_config()).unwrap();
        }

        // Act
        let result = queue.push(audio_config());

        // Assert
        let err = result.unwrap_err();
        assert!(err.to_string().contains("publish queue full"), "{err:#}");
    }

    #[test]
    fn reports_why_publishing_stopped() {
        // Arrange
        let (mut queue, consumer) = PublishQueue::open();
        let QueueConsumer {
            event_receiver,
            failure_sender,
        } = consumer;
        failure_sender.send(anyhow!("relay gone")).unwrap();
        drop(event_receiver);

        // Act
        let result = queue.push(audio());

        // Assert
        let err = result.unwrap_err();
        assert_eq!(format!("{err:#}"), "media publishing stopped: relay gone");
    }
}
