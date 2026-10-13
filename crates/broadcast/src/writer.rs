use anyhow::Result;
use mediapack::MediaEvent;
use tokio::task::JoinHandle;

use crate::{
    broadcast::{Broadcast, SourceTracks},
    publish_queue::{PublishQueue, QueueConsumer},
    track_publisher::{SharedTiming, TrackPublisher},
};

pub struct BroadcastWriter {
    broadcast: Broadcast,
    timing: SharedTiming,
    queue: PublishQueue,
    _publish_task: JoinHandle<()>,
}

impl BroadcastWriter {
    pub fn run(broadcast: Broadcast, tracks: SourceTracks) -> Self {
        let tracks = TrackPublisher::new(broadcast.clone(), tracks);
        let timing = tracks.shared_timing();
        let (queue, consumer) = PublishQueue::open(broadcast.connection().clone());
        Self {
            broadcast,
            timing,
            queue,
            _publish_task: tokio::spawn(publish_queued_events(consumer, tracks)),
        }
    }

    pub fn push(&mut self, event: MediaEvent) -> Result<()> {
        self.queue.push(event)
    }

    pub fn broadcast(&self) -> &Broadcast {
        &self.broadcast
    }

    pub fn shared_timing(&self) -> SharedTiming {
        self.timing.clone()
    }
}

async fn publish_queued_events(mut consumer: QueueConsumer, mut tracks: TrackPublisher) {
    while let Some(event) = consumer.event_receiver.recv().await {
        if let Err(err) = tracks.push(&event).await {
            let _ = consumer.failure_sender.send(err);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use mediapack::{Timestamp, VideoSample};

    use super::*;
    use crate::relay_connection::{MoqtTarget, RelayConnection};

    fn keyframe() -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: Bytes::from_static(b"nal"),
            is_keyframe: true,
            pts: Timestamp::ZERO,
            dts: Timestamp::ZERO,
        })
    }

    #[tokio::test]
    async fn push_reports_the_relay_failure_once_the_publish_task_has_stopped() {
        // Arrange
        let target = MoqtTarget {
            url: "ftp://relay.invalid".to_string(),
            auth_token: None,
            verify_certificate: true,
        };
        let broadcast = Broadcast::new(RelayConnection::new(Some(target)), vec!["ns".to_string()]);
        let mut writer = BroadcastWriter::run(broadcast, SourceTracks::default());

        // Act
        let queued = writer.push(keyframe());
        (&mut writer._publish_task).await.unwrap();
        let failed = writer.push(keyframe());

        // Assert
        assert!(queued.is_ok());
        let err = failed.unwrap_err();
        assert!(
            format!("{err:#}").contains(r#"unsupported connect url scheme "ftp""#),
            "{err:#}"
        );
    }
}
