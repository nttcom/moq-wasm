use anyhow::{Result, anyhow, bail};
use mediapack::MediaEvent;
use tokio::sync::{
    mpsc::{self, error::TrySendError},
    oneshot,
};

use crate::manager::MoqtManager;

/// 30 fps video with 48 kHz AAC audio (1024 samples per frame) is about 77
/// samples per second.
const SAMPLES_PER_SECOND: usize = 77;
/// A relay stall shorter than this is absorbed so the relay cache stays
/// complete for FETCH; live viewers skip the backlog on the next keyframe. A
/// longer stall drops media rather than holding back the ingest, whose SRT or
/// RTMP sender gives up on a connection that is not drained.
const CAPACITY: usize = 30 * SAMPLES_PER_SECOND;
/// Codec configurations are never dropped, because a catalog that misses one
/// leaves viewers unable to decode the media that follows; samples leave this
/// many slots free for them.
const CONFIG_HEADROOM: usize = 8;
const DROP_LOG_INTERVAL: u64 = 500;
/// Queued samples are delivered late by however long they sit here; a backlog
/// this deep is worth attention well before anything is dropped.
const BACKLOG_WARN_DEPTH: usize = 3 * SAMPLES_PER_SECOND;
const BACKLOG_CLEARED_DEPTH: usize = BACKLOG_WARN_DEPTH / 2;

/// Hands media to the publish task without ever waiting for the relay. When
/// the queue is full, samples are dropped, and video stays dropped until a
/// keyframe fits so that no viewer receives a group it cannot decode from its
/// start.
pub(crate) struct PublishQueue {
    moqt: MoqtManager,
    event_sender: mpsc::Sender<MediaEvent>,
    failure_receiver: oneshot::Receiver<anyhow::Error>,
    awaiting_keyframe: bool,
    backlog_reported: bool,
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
    pub(crate) fn open(moqt: MoqtManager) -> (Self, QueueConsumer) {
        let (event_sender, event_receiver) = mpsc::channel(CAPACITY);
        let (failure_sender, failure_receiver) = oneshot::channel();
        (
            Self {
                moqt,
                event_sender,
                failure_receiver,
                awaiting_keyframe: false,
                backlog_reported: false,
                dropped: DroppedSamples::default(),
            },
            QueueConsumer {
                event_receiver,
                failure_sender,
            },
        )
    }

    pub(crate) fn push(&mut self, event: MediaEvent) -> Result<()> {
        self.report_backlog();
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

    fn report_backlog(&mut self) {
        let queued = CAPACITY - self.event_sender.capacity();
        if !self.backlog_reported && queued >= BACKLOG_WARN_DEPTH {
            self.backlog_reported = true;
            tracing::warn!(
                queued,
                capacity = CAPACITY,
                transport = ?self.moqt.transport_stats(),
                "publish queue backlog: the relay is not keeping up, media is delivered late"
            );
        } else if self.backlog_reported && queued <= BACKLOG_CLEARED_DEPTH {
            self.backlog_reported = false;
            tracing::info!(queued, "publish queue backlog drained");
        }
    }

    fn record_drop(&mut self) {
        if !self.awaiting_keyframe {
            tracing::warn!(
                capacity = CAPACITY,
                transport = ?self.moqt.transport_stats(),
                "publish queue full: the relay is not keeping up, dropping media until a keyframe fits"
            );
        }
        self.awaiting_keyframe = true;
        if self.dropped.total().is_multiple_of(DROP_LOG_INTERVAL) {
            tracing::warn!(
                dropped_video = self.dropped.video,
                dropped_audio = self.dropped.audio,
                transport = ?self.moqt.transport_stats(),
                "still dropping media: the publish queue is full"
            );
        }
    }

    fn resume(&mut self) {
        tracing::warn!(
            dropped_video = self.dropped.video,
            dropped_audio = self.dropped.audio,
            transport = ?self.moqt.transport_stats(),
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
        let (mut queue, consumer) = PublishQueue::open(MoqtManager::new(None));
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
    fn reports_a_backlog_once_it_reaches_the_warn_depth() {
        // Arrange
        let (mut queue, _consumer) = PublishQueue::open(MoqtManager::new(None));
        for _ in 0..BACKLOG_WARN_DEPTH {
            queue.push(audio()).unwrap();
        }
        assert!(!queue.backlog_reported);

        // Act
        queue.push(audio()).unwrap();

        // Assert
        assert!(queue.backlog_reported);
    }

    #[test]
    fn clears_the_backlog_report_only_after_the_queue_drains_below_half_the_warn_depth() {
        // Arrange
        let (mut queue, mut consumer) = PublishQueue::open(MoqtManager::new(None));
        for _ in 0..=BACKLOG_WARN_DEPTH {
            queue.push(audio()).unwrap();
        }
        assert!(queue.backlog_reported);

        // Act / Assert: draining to just above the cleared depth keeps the report
        free_slots(&mut consumer, BACKLOG_WARN_DEPTH - BACKLOG_CLEARED_DEPTH);
        queue.push(audio()).unwrap();
        assert!(queue.backlog_reported);

        // Act / Assert: draining to the cleared depth clears it
        free_slots(&mut consumer, 2);
        queue.push(audio()).unwrap();
        assert!(!queue.backlog_reported);
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
        let (mut queue, consumer) = PublishQueue::open(MoqtManager::new(None));
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
