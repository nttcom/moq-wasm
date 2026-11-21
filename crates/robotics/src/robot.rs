mod driver_track_task;
mod robot_task;
#[cfg(test)]
mod test_support;

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use broadcast::Broadcast;
use bytes::Bytes;
use moqt::{
    client,
    publish::{GroupBoundary, OutgoingObject},
};
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};

use crate::{
    arbiter::{ArbiterConfig, HoldReason},
    record::{Record, State, encode_object},
    tracks::TrackKind,
};
use robot_task::{Request, RobotLoop, RobotTask};

/// The broadcast element under the room that the robot publishes.
const ROBOT: &str = "robot";

/// What the application's control loop does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Execute `values` at robot time `at_ms`, never before the moment the
    /// command was issued; `driver` is the namespace that sent them,
    /// relative to the room.
    Apply {
        at_ms: u64,
        values: Vec<f64>,
        driver: String,
    },
    /// Hold still (hold position, stop motors: the application's choice)
    /// until the next `Apply`.
    Hold(HoldReason),
}

/// The robot side of a robot room
/// (`docs/architecture/robotics/architecture.md`, Driving and holding
/// still): it publishes `state`, `control` and `episode` on the robot's
/// broadcast, reads `action` from every broadcast in the room
/// that may drive, and turns them into `Command`s through an `Arbiter` on the
/// robot's wall clock. The application runs the control loop and calls it.
pub struct Robot {
    broadcast: Broadcast,
    state_seq: AtomicU64,
    request_sender: mpsc::UnboundedSender<Request>,
    command_receiver: mpsc::UnboundedReceiver<Command>,
    _task: RobotTask,
}

impl Robot {
    /// Offers `state`, `control` and `episode` on `broadcast`, the
    /// broadcast `<room>/robot` whose connection is `session`'s
    /// (`RelayConnection::with_client_session`, added to `session` as a
    /// handler), and watches the namespaces and tracks under `room`
    /// (`Session::watch_tracks`). Fails when `broadcast` is not
    /// `<room>/robot`, when the tracks cannot be offered, and when the relay
    /// rejects the SUBSCRIBE_NAMESPACE, as it does when `session` already
    /// watches a prefix overlapping `room`.
    pub async fn run(
        session: Arc<client::Session>,
        broadcast: &Broadcast,
        room: Vec<String>,
        config: ArbiterConfig,
    ) -> Result<Self> {
        let expected: Vec<String> = room.iter().cloned().chain([ROBOT.to_string()]).collect();
        ensure!(
            broadcast.namespace() == expected,
            "the robot's broadcast is {}, not {}",
            broadcast.namespace().join("/"),
            expected.join("/")
        );
        let namespace_path = broadcast.namespace().join("/");
        for kind in [TrackKind::State, TrackKind::Control, TrackKind::Episode] {
            broadcast
                .offer_event_timeline(
                    kind.catalog_track(Some(namespace_path.clone())),
                    kind.priority(),
                )
                .await
                .with_context(|| format!("offer {}", kind.name()))?;
        }
        let watch = session.watch_tracks(&room).await?;
        let (request_sender, request_receiver) = mpsc::unbounded_channel();
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let task = RobotTask::run(RobotLoop::new(
            session,
            broadcast.clone(),
            room,
            config,
            watch,
            request_receiver,
            command_sender,
        ));
        Ok(Self {
            broadcast: broadcast.clone(),
            state_seq: AtomicU64::new(0),
            request_sender,
            command_receiver,
            _task: task,
        })
    }

    /// Publishes a `state` record of `fields`, with the next `seq` and `T`
    /// now, as a group of its own. `fields` must not hold `seq`. Fails when
    /// the relay session has ended.
    pub async fn publish_state(&self, fields: Map<String, Value>) -> Result<()> {
        ensure!(!fields.contains_key("seq"), "state fields hold seq");
        let seq = self.state_seq.fetch_add(1, Ordering::Relaxed);
        write_record(
            &self.broadcast,
            TrackKind::State,
            GroupBoundary::Next,
            State { seq, fields },
        )
        .await
    }

    /// Opens an episode with `task` as its instruction and returns its id:
    /// a new `episode` group starts with the record, and with
    /// `policy_enabled` the policy drives whenever no operator does. Fails
    /// while an episode is open and once the relay session has ended.
    pub async fn start_episode(&self, task: String, policy_enabled: bool) -> Result<u64> {
        self.request(|reply_sender| Request::StartEpisode {
            task,
            policy_enabled,
            reply_sender,
        })
        .await
    }

    /// Ends the open episode with an `end` record in its group. Fails when
    /// no episode is open and once the relay session has ended.
    pub async fn end_episode(&self, success: bool) -> Result<()> {
        self.request(|reply_sender| Request::EndEpisode {
            success,
            reply_sender,
        })
        .await
    }

    /// The next command, in order; `None` once the relay session has ended
    /// and its `Hold(Session)` was returned.
    pub async fn next_command(&mut self) -> Option<Command> {
        self.command_receiver.recv().await
    }

    async fn request<T>(
        &self,
        request: impl FnOnce(oneshot::Sender<Result<T>>) -> Request,
    ) -> Result<T> {
        let (reply_sender, reply_receiver) = oneshot::channel();
        self.request_sender
            .send(request(reply_sender))
            .map_err(|_| anyhow::anyhow!("the robot's relay session has ended"))?;
        reply_receiver
            .await
            .context("the robot's relay session has ended")?
    }
}

/// Sends `data` as one record of `kind` on `broadcast`, stamped now.
async fn write_record<D: Serialize>(
    broadcast: &Broadcast,
    kind: TrackKind,
    group: GroupBoundary,
    data: D,
) -> Result<()> {
    let payload = encode_object(&Record { t: now_ms(), data })?;
    broadcast
        .send_object(
            kind.name(),
            OutgoingObject::plain(group, Bytes::from(payload)),
        )
        .await?;
    Ok(())
}

/// The robot's wall clock in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::*;
    use crate::record::{Control, ControlReason, Episode, decode_object};
    use test_support::{TestDriver, TestRobot, TestRoom, next_record, within};

    const ROOM: &str = "anon/lab";
    const ALICE: &str = "anon/lab/operators/alice";

    /// A hold timeout no test reaches unless it waits for the dead man.
    const NEVER_STALE: ArbiterConfig = ArbiterConfig {
        max_action_age_ms: 300,
        hold_timeout_ms: 60_000,
    };

    /// Sends fresh actions every 20 ms, as teleoperation does, until the
    /// robot applies one; the robot may subscribe after the first ones.
    async fn drive_until_applied(robot: &mut TestRobot, driver: &mut TestDriver) -> Command {
        within(async {
            loop {
                driver.send_fresh_action(vec![0.5, -0.25]).await;
                if let Ok(Some(command)) =
                    tokio::time::timeout(Duration::from_millis(20), robot.robot.next_command())
                        .await
                {
                    return command;
                }
            }
        })
        .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_operator_s_fresh_action_is_applied_and_published_as_control() {
        // Arrange
        let mut room = TestRoom::spawn("drive", ROOM).await;
        let mut robot = TestRobot::run(&mut room, NEVER_STALE).await;
        let mut alice = TestDriver::join(&room, ALICE).await;

        // Act
        let command = drive_until_applied(&mut robot, &mut alice).await;
        let control = room.robot_track(TrackKind::Control).await;

        // Assert
        let Command::Apply { values, driver, .. } = command else {
            panic!("expected an applied action, got {command:?}");
        };
        assert_eq!(
            (values, driver.as_str()),
            (vec![0.5, -0.25], "operators/alice")
        );
        assert_eq!(
            next_record::<Control>(&control).await,
            Control {
                driver: Some("operators/alice".to_string()),
                reason: ControlReason::Action,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_candidate_s_tracks_published_after_its_namespace_are_read() {
        // Arrange: the namespace comes before its tracks
        let mut room = TestRoom::spawn("late-tracks", ROOM).await;
        let mut robot = TestRobot::run(&mut room, NEVER_STALE).await;
        let mut alice = TestDriver::announce(&room, ALICE).await;

        // Act
        alice.publish_tracks().await;
        let command = drive_until_applied(&mut robot, &mut alice).await;

        // Assert
        assert!(
            matches!(&command, Command::Apply { driver, .. } if driver == "operators/alice"),
            "got {command:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_operator_whose_namespace_is_withdrawn_leaves_and_the_robot_holds() {
        // Arrange
        let mut room = TestRoom::spawn("left", ROOM).await;
        let mut robot = TestRobot::run(&mut room, NEVER_STALE).await;
        let mut alice = TestDriver::join(&room, ALICE).await;
        drive_until_applied(&mut robot, &mut alice).await;
        let control = room.robot_track(TrackKind::Control).await;
        next_record::<Control>(&control).await;

        // Act
        alice.leave().await;

        // Assert: the actions applied before the namespace went are passed over
        let hold = within(async {
            loop {
                match robot.next_command().await {
                    Command::Apply { .. } => continue,
                    command => return command,
                }
            }
        })
        .await;
        assert_eq!(hold, Command::Hold(HoldReason::Stale));
        assert_eq!(
            next_record::<Control>(&control).await,
            Control {
                driver: None,
                reason: ControlReason::Left,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_driver_without_fresh_actions_stops_driving_after_the_hold_timeout() {
        // Arrange
        let config = ArbiterConfig {
            max_action_age_ms: 300,
            hold_timeout_ms: 300,
        };
        let mut room = TestRoom::spawn("stale", ROOM).await;
        let mut robot = TestRobot::run(&mut room, config).await;
        let mut alice = TestDriver::join(&room, ALICE).await;
        let control = room.robot_track(TrackKind::Control).await;
        drive_until_applied(&mut robot, &mut alice).await;
        next_record::<Control>(&control).await;

        // Act: the actions sent while waiting may still apply
        let mut last_applied = Instant::now();
        let command = within(async {
            loop {
                match robot.next_command().await {
                    Command::Apply { .. } => last_applied = Instant::now(),
                    command => return command,
                }
            }
        })
        .await;

        // Assert
        assert_eq!(command, Command::Hold(HoldReason::Stale));
        assert!(
            last_applied.elapsed() >= Duration::from_millis(250),
            "held {:?} after the last action",
            last_applied.elapsed()
        );
        assert_eq!(
            next_record::<Control>(&control).await,
            Control {
                driver: None,
                reason: ControlReason::Stale,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn published_states_arrive_with_seq_increasing_by_group() {
        // Arrange
        let mut room = TestRoom::spawn("state", ROOM).await;
        let robot = TestRobot::run(&mut room, NEVER_STALE).await;
        let states = room.robot_track(TrackKind::State).await;

        // Act
        for gripper in [0.1, 0.2, 0.3] {
            let fields = json!({"gripper": gripper}).as_object().unwrap().clone();
            robot.robot.publish_state(fields).await.unwrap();
        }

        // Assert: each record is a group on its own stream, so they may arrive in any order
        let mut received = Vec::new();
        for _ in 0..3 {
            let object = within(states.next()).await.unwrap().unwrap();
            let state = decode_object::<State>(&object.payload)
                .unwrap()
                .remove(0)
                .data;
            received.push((object.group_id, state));
        }
        received.sort_by_key(|(group_id, _)| *group_id);
        let seqs: Vec<u64> = received.iter().map(|(_, state)| state.seq).collect();
        assert!(seqs.is_sorted_by(|a, b| a < b), "seqs by group {seqs:?}");
        let grippers: Vec<&Value> = received
            .iter()
            .map(|(_, state)| &state.fields["gripper"])
            .collect();
        assert_eq!(grippers, [&json!(0.1), &json!(0.2), &json!(0.3)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_episode_is_one_group_from_its_start_to_its_end() {
        // Arrange
        let mut room = TestRoom::spawn("episode", ROOM).await;
        let robot = TestRobot::run(&mut room, NEVER_STALE).await;
        let episodes = room.robot_track(TrackKind::Episode).await;

        // Act
        let id = robot
            .robot
            .start_episode("put the cup on the tray".to_string(), false)
            .await
            .unwrap();
        let second_start = robot.robot.start_episode("again".to_string(), false).await;
        robot.robot.end_episode(true).await.unwrap();

        // Assert
        assert!(second_start.is_err(), "an episode was open");
        let start = within(episodes.next()).await.unwrap().unwrap();
        let end = within(episodes.next()).await.unwrap().unwrap();
        assert_eq!(
            (start.group_id, start.object_id + 1),
            (end.group_id, end.object_id)
        );
        assert_eq!(
            decode_object::<Episode>(&start.payload).unwrap()[0].data,
            Episode::Start {
                id,
                task: "put the cup on the tray".to_string()
            }
        );
        assert_eq!(
            decode_object::<Episode>(&end.payload).unwrap()[0].data,
            Episode::End { id, success: true }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_session_holds_and_ends_the_commands() {
        // Arrange
        let mut room = TestRoom::spawn("session", ROOM).await;
        let mut robot = TestRobot::run(&mut room, NEVER_STALE).await;

        // Act
        robot.session.close().await;

        // Assert
        assert_eq!(
            robot.next_command().await,
            Command::Hold(HoldReason::Session)
        );
        assert_eq!(within(robot.robot.next_command()).await, None);
    }
}
