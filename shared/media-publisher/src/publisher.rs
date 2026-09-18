use anyhow::Result;
use mediapack::MediaEvent;
use tokio::task::JoinHandle;

use crate::{
    manager::MoqtManager,
    publish_queue::{PublishQueue, QueueConsumer},
    track_publisher::{SharedTiming, TrackPublisher},
};

pub struct MediaPublisher {
    moqt: MoqtManager,
    namespace: Vec<String>,
    timing: SharedTiming,
    queue: PublishQueue,
    _publish_task: JoinHandle<()>,
}

impl MediaPublisher {
    pub fn run(moqt: MoqtManager, namespace: Vec<String>) -> Self {
        let tracks = TrackPublisher::new(moqt.clone(), namespace.clone());
        let timing = tracks.shared_timing();
        let (queue, consumer) = PublishQueue::open();
        Self {
            moqt,
            namespace,
            timing,
            queue,
            _publish_task: tokio::spawn(publish_queued_events(consumer, tracks)),
        }
    }

    pub fn push(&mut self, event: MediaEvent) -> Result<()> {
        self.queue.push(event)
    }

    pub fn moqt(&self) -> &MoqtManager {
        &self.moqt
    }

    pub fn namespace(&self) -> &[String] {
        &self.namespace
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
    use crate::manager::MoqtTarget;

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
        };
        let mut publisher =
            MediaPublisher::run(MoqtManager::new(Some(target)), vec!["ns".to_string()]);

        // Act
        let queued = publisher.push(keyframe());
        (&mut publisher._publish_task).await.unwrap();
        let failed = publisher.push(keyframe());

        // Assert
        assert!(queued.is_ok());
        let err = failed.unwrap_err();
        assert!(
            format!("{err:#}").contains("unsupported moqt url scheme: ftp"),
            "{err:#}"
        );
    }
}
