use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, bail};
use broadcast::Broadcast;
use moqt::{
    client::{self, GroupStartOptions, PublishRequest, TrackWatch, WatchEvent},
    namespace::NamespaceEvent,
    publish::GroupBoundary,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::MissedTickBehavior,
};

use super::{
    Command,
    driver_track_task::{DriverRecord, DriverTrack, DriverTrackTask},
};
use crate::{
    arbiter::{Arbiter, ArbiterConfig, Output, is_candidate},
    record::{ControlReason, Episode},
    tracks::{TrackKind, now_ms, write_record},
};

/// The dead man is checked at least this often.
const MAX_TICK_PERIOD_MS: u64 = 50;

/// A call of the application that the robot's loop answers.
pub(super) enum Request {
    StartEpisode {
        task: String,
        policy_enabled: bool,
        reply_sender: oneshot::Sender<Result<u64>>,
    },
    EndEpisode {
        success: bool,
        reply_sender: oneshot::Sender<Result<()>>,
    },
}

/// Runs the robot's loop: the only owner of its `Arbiter`.
pub(super) struct RobotTask {
    handle: JoinHandle<()>,
}

impl RobotTask {
    pub(super) fn run(robot: RobotLoop) -> Self {
        Self {
            handle: tokio::spawn(robot.run()),
        }
    }
}

impl Drop for RobotTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Feeds the `Arbiter` what the room sends and the application asks, in one
/// order, and acts on its outputs.
pub(super) struct RobotLoop {
    session: Arc<client::Session>,
    broadcast: Broadcast,
    room: Vec<String>,
    config: ArbiterConfig,
    watch: TrackWatch,
    request_receiver: mpsc::UnboundedReceiver<Request>,
    command_sender: mpsc::UnboundedSender<Command>,
    arbiter: Arbiter,
    /// The candidates whose namespace is published, relative to the room.
    candidates: HashSet<String>,
    /// The `action` track read per candidate; a PUBLISH of the track again
    /// replaces its reader.
    driver_tracks: HashMap<String, DriverTrackTask>,
    record_sender: mpsc::UnboundedSender<DriverRecord>,
    record_receiver: mpsc::UnboundedReceiver<DriverRecord>,
    /// The open episode's id.
    episode: Option<u64>,
    next_episode_id: u64,
}

enum Step {
    Namespace(NamespaceEvent),
    Track(PublishRequest),
    Record(DriverRecord),
    Request(Request),
    Tick,
    SessionEnded,
}

impl RobotLoop {
    pub(super) fn new(
        session: Arc<client::Session>,
        broadcast: Broadcast,
        room: Vec<String>,
        config: ArbiterConfig,
        watch: TrackWatch,
        request_receiver: mpsc::UnboundedReceiver<Request>,
        command_sender: mpsc::UnboundedSender<Command>,
    ) -> Self {
        let (record_sender, record_receiver) = mpsc::unbounded_channel();
        Self {
            session,
            broadcast,
            room,
            config,
            watch,
            request_receiver,
            command_sender,
            arbiter: Arbiter::new(config),
            candidates: HashSet::new(),
            driver_tracks: HashMap::new(),
            record_sender,
            record_receiver,
            episode: None,
            next_episode_id: 1,
        }
    }

    /// Returns once the relay session has ended, after the hold it causes.
    async fn run(mut self) {
        let mut ticks = tokio::time::interval(tick_period(self.config));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let session = self.session.clone();
        let closed = session.closed();
        tokio::pin!(closed);
        loop {
            let step = tokio::select! {
                _ = &mut closed => Step::SessionEnded,
                event = self.watch.next() => match event {
                    Some(WatchEvent::Namespace(event)) => Step::Namespace(event),
                    Some(WatchEvent::Track(request)) => Step::Track(request),
                    None => Step::SessionEnded,
                },
                Some(record) = self.record_receiver.recv() => Step::Record(record),
                Some(request) = self.request_receiver.recv() => Step::Request(request),
                _ = ticks.tick() => Step::Tick,
            };
            match step {
                Step::Namespace(event) => self.namespace(event).await,
                Step::Track(request) => self.track(request).await,
                Step::Record(record) => self.record(record).await,
                Step::Request(request) => self.request(request).await,
                Step::Tick => {
                    let outputs = self.arbiter.tick(now_ms());
                    self.act(outputs).await;
                }
                Step::SessionEnded => break,
            }
        }
        self.driver_tracks.clear();
        self.candidates.clear();
        let outputs = self.arbiter.session_lost(now_ms());
        self.act(outputs).await;
        self.episode = None;
    }

    async fn namespace(&mut self, event: NamespaceEvent) {
        let Some(sender) = relative_to(&self.room, &event.namespace) else {
            return;
        };
        if !is_candidate(&sender) {
            return;
        }
        if event.active {
            if self.candidates.insert(sender) {
                tracing::debug!(track_namespace = %event.namespace.join("/"), "driver candidate joined");
            }
        } else if self.candidates.remove(&sender) {
            tracing::debug!(track_namespace = %event.namespace.join("/"), "driver candidate left");
            self.driver_tracks.remove(&sender);
            let outputs = self.arbiter.left(now_ms(), &sender);
            self.act(outputs).await;
        }
    }

    /// Reads a candidate's `action` from the group its PUBLISH found, at the
    /// track's priority and with the action age as its DELIVERY TIMEOUT;
    /// rejects every other track.
    async fn track(&mut self, request: PublishRequest) {
        let namespace = request.namespace().to_vec();
        let sender = relative_to(&self.room, &namespace).filter(|sender| is_candidate(sender));
        let Some(sender) = sender.filter(|_| request.track() == TrackKind::Action.name()) else {
            if let Err(error) = request.reject().await {
                tracing::debug!(%error, track_namespace = %namespace.join("/"), "PUBLISH not rejected");
            }
            return;
        };
        let options = GroupStartOptions {
            priority: TrackKind::Action.priority(),
            delivery_timeout: Some(Duration::from_millis(self.config.max_action_age_ms)),
            ..GroupStartOptions::default()
        };
        let subscription = match request.accept_current_group(options).await {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::warn!(%error, track_namespace = %namespace.join("/"), track_name = TrackKind::Action.name(), "driver track not accepted");
                return;
            }
        };
        tracing::debug!(track_namespace = %namespace.join("/"), track_name = TrackKind::Action.name(), "driver track read");
        let task = DriverTrackTask::run(DriverTrack {
            subscription,
            namespace,
            sender: sender.clone(),
            record_sender: self.record_sender.clone(),
        });
        self.driver_tracks.insert(sender, task);
    }

    async fn record(&mut self, record: DriverRecord) {
        let outputs = self
            .arbiter
            .action(now_ms(), &record.sender, &record.action);
        if outputs.is_empty() {
            tracing::debug!(
                track_namespace = %format!("{}/{}", self.room.join("/"), record.sender),
                "action dropped"
            );
        }
        self.act(outputs).await;
    }

    async fn request(&mut self, request: Request) {
        match request {
            Request::StartEpisode {
                task,
                policy_enabled,
                reply_sender,
            } => {
                let started = self.start_episode(task, policy_enabled).await;
                let _ = reply_sender.send(started);
            }
            Request::EndEpisode {
                success,
                reply_sender,
            } => {
                let ended = self.end_episode(success).await;
                let _ = reply_sender.send(ended);
            }
        }
    }

    async fn start_episode(&mut self, task: String, policy_enabled: bool) -> Result<u64> {
        if let Some(open) = self.episode {
            bail!("episode {open} is open");
        }
        let id = self.next_episode_id;
        write_record(
            &self.broadcast,
            TrackKind::Episode,
            GroupBoundary::Next,
            Episode::Start { id, task },
        )
        .await?;
        self.next_episode_id += 1;
        self.episode = Some(id);
        tracing::info!(track_namespace = %self.namespace_path(), episode_id = id, "episode started");
        let outputs = self.arbiter.start_episode(now_ms(), policy_enabled);
        self.act(outputs).await;
        Ok(id)
    }

    async fn end_episode(&mut self, success: bool) -> Result<()> {
        let Some(id) = self.episode else {
            bail!("no episode is open");
        };
        write_record(
            &self.broadcast,
            TrackKind::Episode,
            GroupBoundary::Within,
            Episode::End { id, success },
        )
        .await?;
        tracing::info!(track_namespace = %self.namespace_path(), episode_id = id, "episode ended");
        let outputs = self.arbiter.end_episode(now_ms());
        self.episode = None;
        self.act(outputs).await;
        Ok(())
    }

    async fn act(&mut self, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::Apply(applied) => {
                    tracing::debug!(
                        track_namespace = %self.namespace_path(),
                        driver = %applied.driver,
                        loop_latency_ms = applied.loop_latency_ms,
                        "action applied"
                    );
                    let _ = self.command_sender.send(Command::Apply {
                        at_ms: applied.at_ms,
                        values: applied.values,
                        driver: applied.driver,
                    });
                }
                Output::Hold(reason) => {
                    tracing::info!(
                        track_namespace = %self.namespace_path(),
                        driver = self.arbiter.driver(),
                        hold_reason = reason.name(),
                        episode_id = self.episode,
                        "robot holds"
                    );
                    let _ = self.command_sender.send(Command::Hold(reason));
                }
                Output::Control(control) => {
                    // A stale driver is logged by its hold.
                    if control.reason != ControlReason::Stale {
                        tracing::info!(
                            track_namespace = %self.namespace_path(),
                            driver = control.driver.as_deref(),
                            control_reason = control.reason.name(),
                            episode_id = self.episode,
                            "driver changed"
                        );
                    }
                    if let Err(error) = write_record(
                        &self.broadcast,
                        TrackKind::Control,
                        GroupBoundary::Next,
                        control,
                    )
                    .await
                    {
                        tracing::warn!(%error, track_namespace = %self.namespace_path(), "control record not sent");
                    }
                }
            }
        }
    }

    fn namespace_path(&self) -> String {
        self.broadcast.namespace().join("/")
    }
}

/// `namespace` relative to `room`, `None` outside it or for the room itself.
fn relative_to(room: &[String], namespace: &[String]) -> Option<String> {
    namespace
        .strip_prefix(room)
        .filter(|relative| !relative.is_empty())
        .map(|relative| relative.join("/"))
}

/// A tenth of the hold timeout, between 1 ms and `MAX_TICK_PERIOD_MS`, so a
/// hold comes at most 10 % late.
fn tick_period(config: ArbiterConfig) -> Duration {
    Duration::from_millis((config.hold_timeout_ms / 10).clamp(1, MAX_TICK_PERIOD_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elements(path: &str) -> Vec<String> {
        path.split('/').map(str::to_string).collect()
    }

    #[test]
    fn relative_to_strips_the_room_and_skips_other_namespaces() {
        // Arrange
        let room = elements("anon/lab");

        // Act
        let operator = relative_to(&room, &elements("anon/lab/operators/alice"));
        let room_itself = relative_to(&room, &room);
        let other_room = relative_to(&room, &elements("anon/kitchen/operators/alice"));

        // Assert
        assert_eq!(operator.as_deref(), Some("operators/alice"));
        assert_eq!(room_itself, None);
        assert_eq!(other_room, None);
    }

    #[test]
    fn tick_period_is_a_tenth_of_the_hold_timeout_within_its_bounds() {
        // Arrange
        let config = |hold_timeout_ms| ArbiterConfig {
            max_action_age_ms: 300,
            hold_timeout_ms,
        };

        // Act
        let periods = [0, 200, 500, 10_000].map(|hold| tick_period(config(hold)));

        // Assert
        assert_eq!(
            periods,
            [1, 20, 50, 50].map(Duration::from_millis),
            "the default 500 ms hold timeout is checked every 50 ms"
        );
    }
}
