use std::future::Future;

use anyhow::Result;
use mediapack::{AudioSample, MediaEvent, VideoSample};
use moqt::executor::{self, JoinHandle};

use crate::{
    broadcast::{Broadcast, SourceTracks},
    publish_queue::{PublishQueue, QueueConsumer, Queued},
    track_publisher::{SharedTiming, TrackPublisher},
};

pub struct BroadcastWriter {
    broadcast: Broadcast,
    timing: SharedTiming,
    queue: PublishQueue,
    _publish_task: JoinHandle,
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
            _publish_task: executor::spawn(
                "broadcast-publish",
                publish_queued_events(consumer, tracks),
            ),
        }
    }

    pub fn push(&mut self, event: MediaEvent) -> Result<()> {
        // Take the capture origin here: the publish task reaches the first sample
        // only after connecting to the relay, which would stamp every sample into
        // the future.
        if let MediaEvent::Video(VideoSample { pts, .. })
        | MediaEvent::Audio(AudioSample { pts, .. }) = &event
        {
            self.timing.loc_muxer(*pts);
        }
        self.queue.push(event)
    }

    /// `Broadcast::remove_track`, taken after the media pushed before it. The
    /// future does not borrow the writer, so media can be pushed meanwhile.
    pub fn remove_track(&self, track_name: &str) -> impl Future<Output = Result<()>> + 'static {
        self.queue.remove_track(track_name.to_string())
    }

    pub fn broadcast(&self) -> &Broadcast {
        &self.broadcast
    }

    pub fn shared_timing(&self) -> SharedTiming {
        self.timing.clone()
    }
}

async fn publish_queued_events(mut consumer: QueueConsumer, mut tracks: TrackPublisher) {
    while let Some(queued) = consumer.event_receiver.recv().await {
        match queued {
            Queued::Event(event) => {
                if let Err(err) = tracks.push(&event).await {
                    let _ = consumer.failure_sender.send(err);
                    return;
                }
            }
            Queued::RemoveTrack {
                track_name,
                done_sender,
            } => {
                let _ = done_sender.send(tracks.broadcast().remove_track(&track_name).await);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use mediapack::Timestamp;

    use super::*;
    use crate::relay_connection::{MoqtTarget, RelayConnection};

    fn keyframe() -> MediaEvent {
        keyframe_at(Timestamp::ZERO)
    }

    fn keyframe_at(pts: Timestamp) -> MediaEvent {
        MediaEvent::Video(VideoSample {
            data: Bytes::from_static(b"nal"),
            is_keyframe: true,
            pts,
            dts: pts,
        })
    }

    fn wall_clock_micros() -> u64 {
        executor::system_time().as_micros() as u64
    }

    #[tokio::test]
    async fn the_capture_origin_is_taken_when_the_first_sample_is_pushed() {
        // Arrange
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let mut writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let pts = Timestamp::from_micros(1_500_000);

        // Act: without an await the publish task never runs, as while it connects to the relay
        let before = wall_clock_micros();
        writer.push(keyframe_at(pts)).unwrap();
        let after = wall_clock_micros();
        let captured = writer.shared_timing().loc.get().map(|muxer| {
            muxer
                .push(&keyframe_at(pts))
                .unwrap()
                .capture_timestamp()
                .unwrap()
        });

        // Assert
        let captured = captured
            .expect("the push seeds the capture origin")
            .micros();
        assert!(
            (before..=after).contains(&captured),
            "{before} <= {captured} <= {after}"
        );
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
        let err = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::task::yield_now().await;
                if let Err(err) = writer.push(keyframe()) {
                    return err;
                }
            }
        })
        .await
        .expect("the publish task stops on the connect failure");

        // Assert
        assert!(queued.is_ok());
        assert!(
            format!("{err:#}").contains(r#"unsupported connect url scheme "ftp""#),
            "{err:#}"
        );
    }
}
