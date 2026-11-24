use std::sync::Arc;

use mediapack::MediaEvent;
use moqt::executor::{self, JoinHandle};
use tokio::sync::{
    Notify,
    mpsc::{self, error::TrySendError},
    oneshot,
};

use crate::{broadcast::Broadcast, publish_queue::PushOutcome, rendition_track::RenditionTrack};

/// Ten seconds of 30 fps video: a relay stall shorter than this is absorbed,
/// and a longer one drops frames rather than growing the queue without bound.
const CAPACITY: usize = 300;
/// Configurations are never dropped, since the frames after one cannot be
/// decoded without it; frames leave this many slots free for them, and a
/// configuration that finds none is held for the next keyframe that fits.
const CONFIG_HEADROOM: usize = 4;

pub(super) enum RenditionCommand {
    Event(MediaEvent),
    /// `Broadcast::remove_track` for the rendition's track, taken after the
    /// events sent before it.
    Remove(oneshot::Sender<anyhow::Result<()>>),
    /// `Remove`, after which the task stops, so later events fail.
    End(oneshot::Sender<anyhow::Result<()>>),
}

/// Pushes a rendition's events into its track in the order they come, until
/// the track refuses one or the task is dropped. While the track does not
/// keep up and the queue is full, frames are dropped, and then every frame up
/// to a keyframe that fits, so a subscriber gets no group it cannot decode.
pub(super) struct RenditionTask {
    command_sender: mpsc::Sender<RenditionCommand>,
    /// The source's: dropping frames asks the source for a keyframe.
    keyframe_requests: Arc<Notify>,
    awaiting_keyframe: bool,
    /// The latest configuration the full queue did not take, sent before the
    /// next keyframe that fits; the demuxer reports a configuration once.
    held_config: Option<RenditionCommand>,
    track_namespace: String,
    track_name: String,
    join_handle: JoinHandle,
}

impl RenditionTask {
    pub(super) fn run(
        mut track: RenditionTrack,
        broadcast: Broadcast,
        track_name: String,
        keyframe_requests: Arc<Notify>,
    ) -> Self {
        let (command_sender, mut command_receiver) = mpsc::channel(CAPACITY);
        let track_namespace = broadcast.namespace().join("/");
        let join_handle = executor::spawn("media-source-rendition", {
            let track_namespace = track_namespace.clone();
            let track_name = track_name.clone();
            async move {
                while let Some(command) = command_receiver.recv().await {
                    match command {
                        RenditionCommand::Event(event) => {
                            if let Err(error) = track.push(event).await {
                                tracing::warn!(error = %format!("{error:#}"), track_namespace, track_name, "rendition publishing stopped");
                                return;
                            }
                        }
                        RenditionCommand::Remove(done_sender) => {
                            let _ = done_sender.send(broadcast.remove_track(&track_name).await);
                        }
                        RenditionCommand::End(done_sender) => {
                            // Closed before answering, so a push after the end fails at once.
                            command_receiver.close();
                            let _ = done_sender.send(broadcast.remove_track(&track_name).await);
                            return;
                        }
                    }
                }
            }
        });
        Self {
            command_sender,
            keyframe_requests,
            awaiting_keyframe: false,
            held_config: None,
            track_namespace,
            track_name,
            join_handle,
        }
    }

    pub(super) fn command_sender(&self) -> mpsc::Sender<RenditionCommand> {
        self.command_sender.clone()
    }

    /// A configuration held for the next keyframe that fits is `Queued` too.
    /// Fails once the track refused an earlier event, which ended the task.
    pub(super) fn push(&mut self, event: MediaEvent) -> anyhow::Result<PushOutcome> {
        if self.command_sender.is_closed() {
            anyhow::bail!("rendition publishing stopped");
        }
        let has_room = self.command_sender.capacity() > CONFIG_HEADROOM;
        match event {
            MediaEvent::Video(sample) => {
                if !has_room || (self.awaiting_keyframe && !sample.is_keyframe) {
                    if !self.awaiting_keyframe {
                        self.start_dropping();
                    } else if sample.is_keyframe {
                        self.keyframe_requests.notify_one();
                    }
                    return Ok(PushOutcome::Dropped);
                }
                // `has_room` leaves a slot for the held configuration too.
                if let Some(config) = self.held_config.take() {
                    self.send(config)?;
                }
                self.send(RenditionCommand::Event(MediaEvent::Video(sample)))?;
                self.awaiting_keyframe = false;
                Ok(PushOutcome::Queued)
            }
            MediaEvent::VideoConfig(_) if self.held_config.is_some() => {
                self.held_config = Some(RenditionCommand::Event(event));
                Ok(PushOutcome::Queued)
            }
            MediaEvent::VideoConfig(_) => {
                match self.command_sender.try_send(RenditionCommand::Event(event)) {
                    Ok(()) => {}
                    Err(TrySendError::Full(config)) => {
                        self.held_config = Some(config);
                        if !self.awaiting_keyframe {
                            self.start_dropping();
                        }
                    }
                    Err(TrySendError::Closed(_)) => {
                        anyhow::bail!("rendition publishing stopped")
                    }
                }
                Ok(PushOutcome::Queued)
            }
            event => self
                .send(RenditionCommand::Event(event))
                .map(|()| PushOutcome::Queued),
        }
    }

    fn start_dropping(&mut self) {
        tracing::warn!(
            capacity = CAPACITY,
            track_namespace = self.track_namespace,
            track_name = self.track_name,
            "rendition queue full: the relay is not keeping up, dropping frames until a keyframe fits"
        );
        self.awaiting_keyframe = true;
        self.keyframe_requests.notify_one();
    }

    fn send(&self, command: RenditionCommand) -> anyhow::Result<()> {
        match self.command_sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                anyhow::bail!("rendition queue full: the relay is not keeping up")
            }
            Err(TrySendError::Closed(_)) => anyhow::bail!("rendition publishing stopped"),
        }
    }
}

impl Drop for RenditionTask {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use mediapack::{Timestamp, VideoConfig, VideoSample, vp8::Vp8Config};

    use super::*;
    use crate::{
        broadcast::SourceTracks,
        rendition_track::{RenditionConfig, RenditionKeyframes},
        test_support::{FakeRelay, TEST_TIMEOUT},
        writer::BroadcastWriter,
    };

    const FRAME_SLOTS: usize = CAPACITY - CONFIG_HEADROOM;

    /// A rendition task publishing `video_low` through a relay that holds its
    /// PUBLISH_NAMESPACE_OK until the returned sender fires, with the task
    /// already stalled on its first configuration and its queue empty.
    struct StalledRendition {
        relay: FakeRelay,
        task: RenditionTask,
        keyframe_requests: Arc<Notify>,
        accept_namespace: Option<oneshot::Sender<()>>,
        next_pts_us: u64,
    }

    impl StalledRendition {
        async fn start(name: &str) -> Self {
            let (accept_namespace, namespace_gate) = oneshot::channel();
            let mut relay = FakeRelay::spawn_gated(name, Some(namespace_gate), None);
            let broadcast = Broadcast::new(relay.connection(), vec!["live".to_string()]);
            let writer = BroadcastWriter::run(broadcast.clone(), SourceTracks::default());
            let track = RenditionTrack::new(
                &writer,
                RenditionConfig {
                    track: "video_low".to_string(),
                    label: "Low".to_string(),
                    bitrate: None,
                    framerate: None,
                    cmaf: false,
                    keyframes: RenditionKeyframes::SameFrames,
                },
            );
            let keyframe_requests = Arc::new(Notify::new());
            let mut task = RenditionTask::run(
                track,
                broadcast,
                "video_low".to_string(),
                keyframe_requests.clone(),
            );
            task.push(MediaEvent::VideoConfig(vp8_config(64, 48)))
                .unwrap();
            relay.wait_for_request("PUBLISH_NAMESPACE live").await;
            Self {
                relay,
                task,
                keyframe_requests,
                accept_namespace: Some(accept_namespace),
                next_pts_us: 0,
            }
        }

        fn push_frame(&mut self, is_keyframe: bool) -> PushOutcome {
            let pts = Timestamp::from_micros(self.next_pts_us);
            self.next_pts_us += 33_333;
            self.task
                .push(MediaEvent::Video(VideoSample {
                    data: Bytes::from_static(b"frame"),
                    is_keyframe,
                    pts,
                    dts: pts,
                }))
                .unwrap()
        }

        /// Pushes a keyframe and then delta frames until the queue drops one,
        /// returning how many it queued.
        fn fill(&mut self) -> usize {
            let mut queued = 0;
            let mut is_keyframe = true;
            while self.push_frame(is_keyframe) == PushOutcome::Queued {
                queued += 1;
                is_keyframe = false;
                assert!(queued <= CAPACITY, "the queue must be bounded");
            }
            queued
        }

        async fn keyframe_requested(&self) -> bool {
            tokio::time::timeout(Duration::ZERO, self.keyframe_requests.notified())
                .await
                .is_ok()
        }

        /// Lets the relay answer, then waits for the task to take every
        /// queued event.
        async fn resume_and_drain(&mut self) {
            if let Some(accept_namespace) = self.accept_namespace.take() {
                let _ = accept_namespace.send(());
            }
            tokio::time::timeout(TEST_TIMEOUT, async {
                while self.task.command_sender.capacity() < CAPACITY {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the rendition queue must drain once the relay answers");
        }
    }

    fn vp8_config(width: u32, height: u32) -> VideoConfig {
        VideoConfig::Vp8(Vp8Config { width, height })
    }

    #[tokio::test]
    async fn a_stalled_relay_drops_frames_after_the_bound_until_a_keyframe_fits() {
        // Arrange
        let mut stalled = StalledRendition::start("rendition-queue-bound").await;

        // Act
        let queued = stalled.fill();
        let delta_while_full = stalled.push_frame(false);
        let keyframe_while_full = stalled.push_frame(true);
        stalled.resume_and_drain().await;
        let delta_after_drain = stalled.push_frame(false);
        let keyframe_after_drain = stalled.push_frame(true);
        let delta_after_keyframe = stalled.push_frame(false);

        // Assert
        assert_eq!(queued, FRAME_SLOTS);
        assert_eq!(delta_while_full, PushOutcome::Dropped);
        assert_eq!(keyframe_while_full, PushOutcome::Dropped);
        assert_eq!(delta_after_drain, PushOutcome::Dropped);
        assert_eq!(keyframe_after_drain, PushOutcome::Queued);
        assert_eq!(delta_after_keyframe, PushOutcome::Queued);
    }

    #[tokio::test]
    async fn dropping_asks_the_source_for_a_keyframe_and_again_for_each_dropped_keyframe() {
        // Arrange
        let mut stalled = StalledRendition::start("rendition-queue-keyframe-request").await;
        stalled.fill();
        let on_first_drop = stalled.keyframe_requested().await;

        // Act
        stalled.push_frame(false);
        let on_dropped_delta = stalled.keyframe_requested().await;
        stalled.push_frame(true);
        let on_dropped_keyframe = stalled.keyframe_requested().await;

        // Assert
        assert!(on_first_drop);
        assert!(!on_dropped_delta);
        assert!(on_dropped_keyframe);
    }

    #[tokio::test]
    async fn a_configuration_refused_while_full_is_sent_before_the_next_keyframe_that_fits() {
        // Arrange: frames fill the queue and configurations its headroom
        let mut stalled = StalledRendition::start("rendition-queue-held-config").await;
        stalled.fill();
        for _ in 0..CONFIG_HEADROOM {
            stalled
                .task
                .push(MediaEvent::VideoConfig(vp8_config(640, 480)))
                .unwrap();
        }
        let refused = stalled
            .task
            .push(MediaEvent::VideoConfig(vp8_config(1280, 720)))
            .unwrap();
        stalled.resume_and_drain().await;

        // Act
        let keyframe = stalled.push_frame(true);
        let catalog = stalled.relay.newest_catalog().await;

        // Assert
        assert_eq!(refused, PushOutcome::Queued);
        assert_eq!(keyframe, PushOutcome::Queued);
        let tracks = catalog.tracks;
        let low = tracks
            .iter()
            .find(|track| track.name == "video_low")
            .expect("the rendition must be listed");
        assert_eq!((low.width, low.height), (Some(1280), Some(720)));
    }
}
