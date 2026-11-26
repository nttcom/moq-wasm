use std::future::Future;

use anyhow::{Result, bail};
use mediapack::{AudioSample, MediaEvent, Timestamp, VideoSample};
use moqt::executor::{self, JoinHandle};

use crate::{
    broadcast::{Broadcast, SourceTracks},
    publish_queue::{PublishQueue, PushOutcome, QueueConsumer, Queued},
    track_publisher::{SharedTiming, TrackPublisher},
};

pub struct BroadcastWriter {
    broadcast: Broadcast,
    source_tracks: SourceTracks,
    timing: SharedTiming,
    queue: PublishQueue,
    _publish_task: JoinHandle,
}

impl BroadcastWriter {
    pub fn run(broadcast: Broadcast, source_tracks: SourceTracks) -> Self {
        let tracks = TrackPublisher::new(broadcast.clone(), source_tracks.clone());
        let timing = tracks.shared_timing();
        let (queue, consumer) = PublishQueue::open(
            broadcast.connection().clone(),
            source_tracks.keyframe_requests.clone(),
        );
        Self {
            broadcast,
            source_tracks,
            timing,
            queue,
            _publish_task: executor::spawn(
                "broadcast-publish",
                publish_queued_events(consumer, tracks),
            ),
        }
    }

    /// Declares `origin` as the wall-clock time (Unix µs) of presentation time
    /// zero, for a source that knows when its samples were captured. Without
    /// it the first pushed sample counts as captured when it is pushed. Fails
    /// once a sample was pushed or an origin was set.
    pub fn set_capture_origin(&mut self, origin: Timestamp) -> Result<()> {
        if !self.timing.seed_capture_origin(origin) {
            bail!("the capture origin is already taken");
        }
        Ok(())
    }

    /// Queues `event` for the publish task without waiting for the relay,
    /// or drops it as `PushOutcome::Dropped` says. Fails once the publish
    /// task stopped, and when codec configurations fill even the slots kept
    /// free for them.
    pub fn push(&mut self, event: MediaEvent) -> Result<PushOutcome> {
        // Take the capture origin here: the publish task reaches the first sample
        // only after connecting to the relay, which would stamp every sample into
        // the future.
        if let MediaEvent::Video(VideoSample { pts, .. })
        | MediaEvent::Audio(AudioSample { pts, .. }) = &event
        {
            self.timing.capture_origin(*pts);
        }
        // Number a keyframe's group here: the publish task reaches the sample
        // only after the relay takes those queued before, and a `FollowSource`
        // rendition looks the id up meanwhile.
        if let MediaEvent::Video(VideoSample {
            pts,
            is_keyframe: true,
            ..
        }) = &event
        {
            self.timing.alignment.keyframe(pts.micros());
        }
        self.queue.push(event)
    }

    /// `Broadcast::remove_track`, taken after the media pushed before it. The
    /// future does not borrow the writer, so media can be pushed meanwhile.
    pub fn remove_track(&self, track_name: &str) -> impl Future<Output = Result<()>> + use<> {
        self.queue.remove_track(track_name.to_string())
    }

    /// `remove_track`, or `Broadcast::remove_track` at once when the publish
    /// task has stopped, as after the relay refused a write.
    pub(crate) fn end_track(&self, track_name: &str) -> impl Future<Output = Result<()>> + use<> {
        remove_track_after(
            self.queue.queue_removal(track_name.to_string()),
            self.broadcast.clone(),
            track_name.to_string(),
        )
    }

    pub fn source_tracks(&self) -> &SourceTracks {
        &self.source_tracks
    }

    pub fn broadcast(&self) -> &Broadcast {
        &self.broadcast
    }

    pub(crate) fn shared_timing(&self) -> SharedTiming {
        self.timing.clone()
    }
}

/// Awaits `removal`, sent to the task that writes the track, or removes the
/// track from `broadcast` at once when that task has stopped and so never
/// takes it.
pub(crate) async fn remove_track_after(
    removal: impl Future<Output = Option<Result<()>>>,
    broadcast: Broadcast,
    track_name: String,
) -> Result<()> {
    match removal.await {
        Some(removed) => removed,
        None => broadcast.remove_track(&track_name).await,
    }
}

async fn publish_queued_events(mut consumer: QueueConsumer, mut tracks: TrackPublisher) {
    while let Some(queued) = consumer.event_receiver.recv().await {
        match queued {
            Queued::Event(event) => {
                if let Err(err) = tracks.push(&event).await {
                    // The oneshot hands the cause to one later push; the pushes after it get a bare
                    // "media publishing stopped", so log the cause here.
                    tracing::error!(error = %format!("{err:#}"), "media publishing stopped");
                    let _ = consumer.failure_sender.send(err);
                    return;
                }
            }
            Queued::RemoveTrack {
                track_name,
                done_sender,
            } => {
                let _ = done_sender.send(tracks.remove_track(&track_name).await);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::relay_connection::{MoqtTarget, RelayConnection};
    use crate::test_support::keyframe_at;

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
        writer.push(keyframe_at(pts.micros())).unwrap();
        let after = wall_clock_micros();
        let captured = writer
            .shared_timing()
            .seeded_capture_origin()
            .map(|origin| origin.saturating_add(pts));

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
    async fn a_capture_origin_set_before_the_first_sample_replaces_the_arrival_time() {
        // Arrange
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let mut writer = BroadcastWriter::run(broadcast, SourceTracks::default());
        let origin = Timestamp::from_micros(1_000_000);

        // Act
        writer.set_capture_origin(origin).unwrap();
        writer.push(keyframe_at(1_500_000)).unwrap();

        // Assert
        assert_eq!(writer.shared_timing().seeded_capture_origin(), Some(origin));
    }

    #[tokio::test]
    async fn setting_the_capture_origin_fails_once_a_sample_was_pushed_or_an_origin_was_set() {
        // Arrange
        let broadcast = Broadcast::new(RelayConnection::new(None), vec!["ns".to_string()]);
        let mut pushed = BroadcastWriter::run(broadcast.clone(), SourceTracks::default());
        pushed.push(keyframe_at(0)).unwrap();
        let taken = pushed.shared_timing().seeded_capture_origin();
        let mut set = BroadcastWriter::run(broadcast, SourceTracks::default());
        set.set_capture_origin(Timestamp::from_micros(1)).unwrap();

        // Act
        let after_a_push = pushed.set_capture_origin(Timestamp::from_micros(2));
        let after_a_set = set.set_capture_origin(Timestamp::from_micros(2));

        // Assert: the origin taken first stays
        assert!(after_a_push.is_err());
        assert!(after_a_set.is_err());
        assert_eq!(pushed.shared_timing().seeded_capture_origin(), taken);
        assert_eq!(
            set.shared_timing().seeded_capture_origin(),
            Some(Timestamp::from_micros(1))
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
        let queued = writer.push(keyframe_at(0));
        let err = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::task::yield_now().await;
                if let Err(err) = writer.push(keyframe_at(0)) {
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
