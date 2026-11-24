use std::collections::HashMap;

use crate::record::{Action, Control, ControlReason};

/// The policy bot's namespace relative to the room.
const POLICY: &str = "bots/policy";
const OPERATORS: &str = "operators/";

/// The durations of `docs/architecture/robotics/architecture.md`, Driving
/// and holding still.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArbiterConfig {
    /// The oldest `now − basis` an applied action may have.
    pub max_action_age_ms: u64,
    /// How long the driver may go without an applied action before the
    /// robot holds.
    pub hold_timeout_ms: u64,
}

impl Default for ArbiterConfig {
    fn default() -> Self {
        Self {
            max_action_age_ms: 300,
            hold_timeout_ms: 500,
        }
    }
}

/// What the robot does after an event, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// Execute one action entry.
    Apply(Applied),
    /// Publish this `control` record.
    Control(Control),
    /// Hold still (hold position, stop motors: the application's choice)
    /// until the next `Apply`.
    Hold(HoldReason),
}

/// One entry of an applied action chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub driver: String,
    /// The robot time the entry is meant for, never before the call's
    /// `now_ms`.
    pub at_ms: u64,
    pub values: Vec<f64>,
    /// `now − basis` of the action.
    pub loop_latency_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// Nobody drives, or the driver applied no action for `hold_timeout_ms`.
    Stale,
    /// The robot's session to the relay ended.
    Session,
}

impl HoldReason {
    /// The `hold_reason` log field's value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::Session => "session",
        }
    }
}

/// Arbitration and the dead-man rule on the robot
/// (`docs/architecture/robotics/architecture.md`, Driving and holding
/// still). It has no clock: every call takes `now_ms`, the robot's wall
/// clock in milliseconds, and returns what to do. A `sender` is a namespace
/// relative to the room; only `operators/<id>` and `bots/policy` are
/// candidates, and the calls ignore any other.
#[derive(Debug, Clone)]
pub struct Arbiter {
    config: ArbiterConfig,
    /// The operator driving: the one whose fresh action applied while nobody
    /// or the policy drove, until its actions go stale or it leaves.
    operator: Option<String>,
    /// An episode with the policy enabled is open and the policy has not
    /// left during it.
    policy_may_drive: bool,
    /// Per sender, the `seq` of its last applied action.
    last_seqs: HashMap<String, u64>,
    /// The hold in force: one was emitted and no action applied since.
    hold: Option<HoldReason>,
    /// When the driver last applied an action or became driver.
    armed_ms: u64,
    /// The dead man fired for the current driver since `armed_ms`.
    stale: bool,
}

impl Arbiter {
    /// An arbiter with nobody driving and no episode open.
    pub fn new(config: ArbiterConfig) -> Self {
        Self {
            config,
            operator: None,
            policy_may_drive: false,
            last_seqs: HashMap::new(),
            hold: None,
            armed_ms: 0,
            stale: false,
        }
    }

    /// The namespace driving: an operator, else the policy during an episode
    /// that enables it, else nobody.
    pub fn driver(&self) -> Option<&str> {
        self.operator
            .as_deref()
            .or(self.policy_may_drive.then_some(POLICY))
    }

    /// `sender`'s namespace was withdrawn: it stops driving and its `seq`
    /// is forgotten, so a publisher that comes back may start over. The
    /// policy does not drive again before the next `start_episode`.
    pub fn left(&mut self, now_ms: u64, sender: &str) -> Vec<Output> {
        let before = self.driver_owned();
        if is_operator(sender) {
            if self.operator.as_deref() == Some(sender) {
                self.operator = None;
            }
        } else if sender == POLICY {
            self.policy_may_drive = false;
        } else {
            return Vec::new();
        }
        self.last_seqs.remove(sender);
        self.settle(now_ms, before, ControlReason::Left)
    }

    /// Applies `action` when `sender` may drive, its `seq` is above the last
    /// applied from `sender`, and `0 ≤ now − basis ≤ max_action_age_ms`:
    /// one `Apply` per entry whose time is not before `now_ms`, in order. The
    /// driver may drive, and so may an operator while nobody or the policy
    /// drives, which makes it the driver (`control` with reason `action`).
    /// An action left with no such entry moved nothing and changes nothing,
    /// neither the driver, its sender's `seq` nor the dead man.
    pub fn action(&mut self, now_ms: u64, sender: &str, action: &Action) -> Vec<Output> {
        let may_drive = match self.driver() {
            Some(driver) => driver == sender || (driver == POLICY && is_operator(sender)),
            None => is_operator(sender),
        };
        if !may_drive
            || self
                .last_seqs
                .get(sender)
                .is_some_and(|&last| action.seq <= last)
            || action.basis > now_ms
            || now_ms - action.basis > self.config.max_action_age_ms
        {
            return Vec::new();
        }
        let loop_latency_ms = now_ms - action.basis;
        let applies: Vec<Output> = action
            .actions
            .iter()
            .enumerate()
            .filter_map(|(index, values)| {
                let at_ms = action.entry_time(index).filter(|&at_ms| at_ms >= now_ms)?;
                Some(Output::Apply(Applied {
                    driver: sender.to_string(),
                    at_ms,
                    values: values.clone(),
                    loop_latency_ms,
                }))
            })
            .collect();
        if applies.is_empty() {
            return Vec::new();
        }
        let mut outputs = Vec::new();
        if self.driver() != Some(sender) {
            let before = self.driver_owned();
            self.operator = Some(sender.to_string());
            outputs = self.settle(now_ms, before, ControlReason::Action);
        }
        self.last_seqs.insert(sender.to_string(), action.seq);
        self.hold = None;
        self.armed_ms = now_ms;
        self.stale = false;
        outputs.extend(applies);
        outputs
    }

    /// Opens an episode. With `policy_enabled` the policy drives whenever no
    /// operator does until `end_episode`; an episode already open is
    /// replaced. Emits a `control` even when the driver stays, so that the
    /// records of an episode start with its driver.
    pub fn start_episode(&mut self, now_ms: u64, policy_enabled: bool) -> Vec<Output> {
        let before = self.driver_owned();
        self.policy_may_drive = policy_enabled;
        let mut outputs = self.settle(now_ms, before, ControlReason::Episode);
        if outputs.is_empty() {
            outputs.push(Output::Control(Control {
                driver: self.driver_owned(),
                reason: ControlReason::Episode,
            }));
        }
        outputs
    }

    /// Closes the episode: the policy stops driving.
    pub fn end_episode(&mut self, now_ms: u64) -> Vec<Output> {
        let before = self.driver_owned();
        self.policy_may_drive = false;
        self.settle(now_ms, before, ControlReason::Episode)
    }

    /// The dead man: once the driver has applied no action for
    /// `hold_timeout_ms` since its last applied one or since it became
    /// driver, holds and publishes `control` with reason `stale`. An
    /// operator stops driving, handing back to the policy during an episode
    /// that enables it; the policy stays the driver and holds once until it
    /// applies an action.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Output> {
        let Some(driver) = self.driver_owned() else {
            return Vec::new();
        };
        if self.stale || now_ms.saturating_sub(self.armed_ms) < self.config.hold_timeout_ms {
            return Vec::new();
        }
        let mut outputs = self.hold_once(HoldReason::Stale);
        if is_operator(&driver) {
            self.operator = None;
            self.armed_ms = now_ms;
        } else {
            self.stale = true;
        }
        outputs.push(Output::Control(Control {
            driver: self.driver_owned(),
            reason: ControlReason::Stale,
        }));
        outputs
    }

    /// The robot's session to the relay ended: holds at once, unless it
    /// already holds for this reason, and forgets the drivers, the episode
    /// and every sender's `seq`. It emits no `control`, which could not be
    /// published.
    pub fn session_lost(&mut self, _now_ms: u64) -> Vec<Output> {
        self.operator = None;
        self.policy_may_drive = false;
        self.last_seqs.clear();
        self.stale = false;
        if self.hold == Some(HoldReason::Session) {
            return Vec::new();
        }
        self.hold = Some(HoldReason::Session);
        vec![Output::Hold(HoldReason::Session)]
    }

    fn driver_owned(&self) -> Option<String> {
        self.driver().map(str::to_string)
    }

    /// The outputs of a driver change from `before`: a hold when nobody
    /// drives any more, then the `control` record.
    fn settle(
        &mut self,
        now_ms: u64,
        before: Option<String>,
        reason: ControlReason,
    ) -> Vec<Output> {
        let after = self.driver_owned();
        if after == before {
            return Vec::new();
        }
        self.armed_ms = now_ms;
        self.stale = false;
        let mut outputs = if after.is_none() {
            self.hold_once(HoldReason::Stale)
        } else {
            Vec::new()
        };
        outputs.push(Output::Control(Control {
            driver: after,
            reason,
        }));
        outputs
    }

    fn hold_once(&mut self, reason: HoldReason) -> Vec<Output> {
        if self.hold.is_some() {
            return Vec::new();
        }
        self.hold = Some(reason);
        vec![Output::Hold(reason)]
    }
}

/// Whether `sender` may drive: `operators/<id>` or `bots/policy`.
pub(crate) fn is_candidate(sender: &str) -> bool {
    is_operator(sender) || sender == POLICY
}

/// `operators/<id>`: one non-empty element after `operators`.
fn is_operator(sender: &str) -> bool {
    sender
        .strip_prefix(OPERATORS)
        .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "operators/alice";
    const BOB: &str = "operators/bob";

    fn arbiter() -> Arbiter {
        Arbiter::new(ArbiterConfig::default())
    }

    fn control(driver: Option<&str>, reason: ControlReason) -> Output {
        Output::Control(Control {
            driver: driver.map(str::to_string),
            reason,
        })
    }

    fn chunk(seq: u64, basis: u64, dt_ms: u64, entries: usize) -> Action {
        Action {
            seq,
            basis,
            dt_ms,
            actions: (0..entries).map(|entry| vec![entry as f64]).collect(),
        }
    }

    fn apply(driver: &str, at_ms: u64, entry: usize, loop_latency_ms: u64) -> Output {
        Output::Apply(Applied {
            driver: driver.to_string(),
            at_ms,
            values: vec![entry as f64],
            loop_latency_ms,
        })
    }

    /// An arbiter `driver` drives since time 1000: the policy from the start
    /// of an episode enabling it, an operator from its action of `seq` 0.
    fn driven_by(driver: &str) -> Arbiter {
        let mut arbiter = arbiter();
        if driver == POLICY {
            arbiter.start_episode(1000, true);
        } else {
            arbiter.action(1000, driver, &chunk(0, 1000, 0, 1));
        }
        arbiter
    }

    #[test]
    fn default_config_is_the_designs_starting_point() {
        // Act / Assert
        assert_eq!(
            ArbiterConfig::default(),
            ArbiterConfig {
                max_action_age_ms: 300,
                hold_timeout_ms: 500
            }
        );
    }

    #[test]
    fn senders_that_are_not_candidates_are_ignored() {
        // Arrange
        let mut arbiter = arbiter();
        let senders = [
            "viewers/carol",
            "operators",
            "operators/",
            "operators/a/b",
            "bots/policy/x",
            "bots/other",
        ];

        // Act
        let outputs: Vec<Output> = senders
            .iter()
            .flat_map(|sender| {
                let mut outputs = arbiter.action(1000, sender, &chunk(1, 1000, 0, 1));
                outputs.extend(arbiter.left(1000, sender));
                outputs
            })
            .collect();

        // Assert
        assert_eq!((outputs, arbiter.driver()), (Vec::new(), None));
    }

    #[test]
    fn an_operators_fresh_action_takes_the_robot() {
        // Arrange
        let mut arbiter = arbiter();

        // Act
        let outputs = arbiter.action(1000, ALICE, &chunk(1, 1000, 0, 1));

        // Assert
        assert_eq!(
            outputs,
            [
                control(Some(ALICE), ControlReason::Action),
                apply(ALICE, 1000, 0, 0)
            ]
        );
        assert_eq!(arbiter.driver(), Some(ALICE));
    }

    #[test]
    fn another_operators_action_is_dropped_while_one_drives() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act / Assert
        assert_eq!(
            (
                arbiter.action(1010, BOB, &chunk(1, 1010, 0, 1)),
                arbiter.driver()
            ),
            (Vec::new(), Some(ALICE))
        );
    }

    #[test]
    fn an_action_with_no_entry_left_takes_nothing() {
        // Arrange
        let mut arbiter = arbiter();

        // Act / Assert: its only entry, at 1020, has passed
        assert_eq!(
            (
                arbiter.action(1100, ALICE, &chunk(1, 1000, 20, 1)),
                arbiter.driver()
            ),
            (Vec::new(), None)
        );
    }

    #[test]
    fn the_drivers_left_stops_it_driving() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act
        let outputs = arbiter.left(1010, ALICE);

        // Assert
        assert_eq!(
            outputs,
            [
                Output::Hold(HoldReason::Stale),
                control(None, ControlReason::Left)
            ]
        );
    }

    #[test]
    fn the_policy_drives_from_the_start_of_an_episode_enabling_it() {
        // Arrange
        let mut arbiter = arbiter();

        // Act
        let outputs = arbiter.start_episode(1000, true);

        // Assert
        assert_eq!(outputs, [control(Some(POLICY), ControlReason::Episode)]);
        assert_eq!(arbiter.driver(), Some(POLICY));
    }

    #[test]
    fn an_episode_without_the_policy_reports_that_nobody_drives() {
        // Arrange
        let mut arbiter = arbiter();

        // Act / Assert
        assert_eq!(
            (arbiter.start_episode(1000, false), arbiter.driver()),
            (vec![control(None, ControlReason::Episode)], None)
        );
    }

    #[test]
    fn an_episode_start_keeps_the_operator_driving_and_reports_it() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act / Assert
        assert_eq!(
            (arbiter.start_episode(1010, true), arbiter.driver()),
            (
                vec![control(Some(ALICE), ControlReason::Episode)],
                Some(ALICE)
            )
        );
    }

    #[test]
    fn an_operators_action_intervenes_and_its_staleness_returns_to_the_policy() {
        // Arrange
        let mut arbiter = driven_by(POLICY);

        // Act
        let intervention = arbiter.action(1010, ALICE, &chunk(1, 1010, 0, 1));
        let stale = arbiter.tick(1510);

        // Assert
        assert_eq!(
            intervention,
            [
                control(Some(ALICE), ControlReason::Action),
                apply(ALICE, 1010, 0, 0)
            ]
        );
        assert_eq!(
            stale,
            [
                Output::Hold(HoldReason::Stale),
                control(Some(POLICY), ControlReason::Stale)
            ]
        );
    }

    #[test]
    fn the_drivers_left_during_an_episode_returns_to_the_policy() {
        // Arrange
        let mut arbiter = driven_by(POLICY);
        arbiter.action(1010, ALICE, &chunk(1, 1010, 0, 1));

        // Act / Assert
        assert_eq!(
            arbiter.left(1020, ALICE),
            [control(Some(POLICY), ControlReason::Left)]
        );
    }

    #[test]
    fn the_end_of_the_episode_stops_the_policy_and_holds() {
        // Arrange
        let mut arbiter = driven_by(POLICY);

        // Act
        let outputs = arbiter.end_episode(1010);

        // Assert
        assert_eq!(
            outputs,
            [
                Output::Hold(HoldReason::Stale),
                control(None, ControlReason::Episode)
            ]
        );
    }

    #[test]
    fn the_policys_left_stops_it_until_the_next_episode() {
        // Arrange
        let mut arbiter = driven_by(POLICY);

        // Act
        let left = arbiter.left(1010, POLICY);
        arbiter.action(1020, ALICE, &chunk(1, 1020, 0, 1));
        arbiter.tick(1520);
        let driver_after_stale = arbiter.driver().map(str::to_string);
        let next_episode = arbiter.start_episode(1530, true);

        // Assert
        assert_eq!(
            left,
            [
                Output::Hold(HoldReason::Stale),
                control(None, ControlReason::Left)
            ]
        );
        assert_eq!(driver_after_stale, None);
        assert_eq!(
            next_episode,
            [control(Some(POLICY), ControlReason::Episode)]
        );
    }

    #[test]
    fn a_fresh_action_applies_each_entry_at_its_time() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act
        let outputs = arbiter.action(1100, ALICE, &chunk(1, 1060, 20, 3));

        // Assert: entry 0 at 1080 has passed
        assert_eq!(
            outputs,
            [apply(ALICE, 1100, 1, 40), apply(ALICE, 1120, 2, 40)]
        );
    }

    #[test]
    fn an_action_exactly_max_action_age_old_applies_and_one_older_is_dropped() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act
        let older = arbiter.action(1300, ALICE, &chunk(1, 999, 1, 2));
        let exact = arbiter.action(1300, ALICE, &chunk(2, 1000, 300, 1));

        // Assert
        assert_eq!(older, []);
        assert_eq!(exact, [apply(ALICE, 1300, 0, 300)]);
    }

    #[test]
    fn an_action_with_a_basis_in_the_future_is_dropped() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act / Assert
        assert_eq!(arbiter.action(1100, ALICE, &chunk(1, 1101, 20, 2)), []);
    }

    #[test]
    fn an_action_with_a_seq_not_above_the_last_applied_is_dropped() {
        // Arrange
        let mut arbiter = driven_by(ALICE);
        arbiter.action(1100, ALICE, &chunk(5, 1100, 0, 1));

        // Act
        let repeated = arbiter.action(1110, ALICE, &chunk(5, 1110, 0, 1));
        let stepped_back = arbiter.action(1110, ALICE, &chunk(4, 1110, 0, 1));

        // Assert
        assert_eq!((repeated, stepped_back), (Vec::new(), Vec::new()));
    }

    #[test]
    fn an_entry_whose_time_overflows_is_skipped() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act
        let outputs = arbiter.action(1100, ALICE, &chunk(1, 1100, u64::MAX - 1100, 2));

        // Assert: entry 1 would be at 1100 + 2 × (u64::MAX - 1100)
        assert_eq!(outputs, [apply(ALICE, u64::MAX, 0, 0)]);
    }

    #[test]
    fn an_action_with_no_entry_left_changes_nothing() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act
        let passed = arbiter.action(1100, ALICE, &chunk(7, 1000, 20, 2));
        let same_seq = arbiter.action(1100, ALICE, &chunk(7, 1100, 0, 1));
        let dead_man = arbiter.tick(1500);

        // Assert: seq 7 was not taken, and the dead man counts from 1000
        assert_eq!(passed, []);
        assert_eq!(same_seq, [apply(ALICE, 1100, 0, 0)]);
        assert_eq!(dead_man, []);
        assert_eq!(arbiter.tick(1600).len(), 2);
    }

    #[test]
    fn an_action_from_a_candidate_that_does_not_drive_is_dropped() {
        // Arrange
        let mut arbiter = driven_by(ALICE);
        arbiter.start_episode(1000, true);

        // Act / Assert
        assert_eq!(
            (
                arbiter.action(1100, BOB, &chunk(1, 1100, 0, 1)),
                arbiter.action(1100, POLICY, &chunk(1, 1100, 0, 1))
            ),
            (Vec::new(), Vec::new())
        );
    }

    #[test]
    fn a_stale_operator_stops_driving_and_the_robot_holds() {
        // Arrange
        let mut arbiter = driven_by(ALICE);

        // Act
        let before = arbiter.tick(1499);
        let at = arbiter.tick(1500);
        let after = arbiter.tick(2500);

        // Assert
        assert_eq!(before, []);
        assert_eq!(
            at,
            [
                Output::Hold(HoldReason::Stale),
                control(None, ControlReason::Stale)
            ]
        );
        assert_eq!((after, arbiter.driver()), (Vec::new(), None));
    }

    #[test]
    fn the_dead_man_holds_the_policy_once_and_keeps_it_driving() {
        // Arrange
        let mut arbiter = driven_by(POLICY);

        // Act
        let at = arbiter.tick(1500);
        let after = arbiter.tick(2500);

        // Assert
        assert_eq!(
            at,
            [
                Output::Hold(HoldReason::Stale),
                control(Some(POLICY), ControlReason::Stale)
            ]
        );
        assert_eq!((after, arbiter.driver()), (Vec::new(), Some(POLICY)));
    }

    #[test]
    fn the_policys_next_applied_action_resumes_and_rearms_the_dead_man() {
        // Arrange
        let mut arbiter = driven_by(POLICY);
        arbiter.tick(1500);

        // Act
        let resumed = arbiter.action(1600, POLICY, &chunk(1, 1600, 0, 1));
        let before = arbiter.tick(2099);
        let at = arbiter.tick(2100);

        // Assert
        assert_eq!(resumed, [apply(POLICY, 1600, 0, 0)]);
        assert_eq!(before, []);
        assert_eq!(
            at,
            [
                Output::Hold(HoldReason::Stale),
                control(Some(POLICY), ControlReason::Stale)
            ]
        );
    }

    #[test]
    fn another_operator_may_drive_once_the_driver_went_stale() {
        // Arrange
        let mut arbiter = driven_by(ALICE);
        arbiter.tick(1500);

        // Act / Assert
        assert_eq!(
            arbiter.action(1600, BOB, &chunk(1, 1600, 0, 1)),
            [
                control(Some(BOB), ControlReason::Action),
                apply(BOB, 1600, 0, 0)
            ]
        );
    }

    #[test]
    fn the_dead_man_is_silent_while_nobody_drives() {
        // Arrange
        let mut arbiter = arbiter();

        // Act / Assert
        assert_eq!(arbiter.tick(u64::MAX), []);
    }

    #[test]
    fn session_loss_holds_once_and_forgets_drivers_and_seqs() {
        // Arrange
        let mut arbiter = driven_by(ALICE);
        arbiter.start_episode(1000, true);
        arbiter.action(1100, ALICE, &chunk(9, 1100, 0, 1));

        // Act
        let lost = arbiter.session_lost(1110);
        let lost_again = arbiter.session_lost(1120);
        let restarted = arbiter.action(1200, ALICE, &chunk(1, 1200, 0, 1));

        // Assert
        assert_eq!(lost, [Output::Hold(HoldReason::Session)]);
        assert_eq!(lost_again, []);
        assert_eq!(
            restarted,
            [
                control(Some(ALICE), ControlReason::Action),
                apply(ALICE, 1200, 0, 0)
            ]
        );
    }

    #[test]
    fn session_loss_while_holding_stale_still_reports_it() {
        // Arrange
        let mut arbiter = driven_by(ALICE);
        arbiter.tick(1500);

        // Act / Assert
        assert_eq!(
            arbiter.session_lost(1510),
            [Output::Hold(HoldReason::Session)]
        );
    }

    #[test]
    fn left_forgets_the_senders_seq() {
        // Arrange
        let mut arbiter = driven_by(ALICE);
        arbiter.action(1100, ALICE, &chunk(9, 1100, 0, 1));
        arbiter.left(1110, ALICE);

        // Act / Assert
        assert_eq!(
            arbiter.action(1130, ALICE, &chunk(1, 1130, 0, 1)),
            [
                control(Some(ALICE), ControlReason::Action),
                apply(ALICE, 1130, 0, 0)
            ]
        );
    }

    mod properties {
        use std::collections::HashMap;

        use proptest::collection::vec;
        use proptest::prelude::*;

        use super::*;

        const CONFIG: ArbiterConfig = ArbiterConfig {
            max_action_age_ms: 300,
            hold_timeout_ms: 500,
        };

        #[derive(Debug, Clone)]
        enum Basis {
            Back(u64),
            Ahead(u64),
            At(u64),
        }

        /// The `Left` sender standing for whoever drives when it happens, so
        /// that the driver leaves often; `run` traces the sender it resolves.
        const THE_DRIVER: &str = "the driver";

        #[derive(Debug, Clone)]
        enum Event {
            Left(&'static str),
            /// `sender: None` is whoever drives when the action arrives, so
            /// that the driver's actions, whose `seq` and freshness matter,
            /// are frequent.
            Action {
                sender: Option<&'static str>,
                seq: u64,
                basis: Basis,
                dt_ms: u64,
                entries: usize,
            },
            StartEpisode(bool),
            EndEpisode,
            Tick,
            SessionLost,
        }

        /// One call: the clock moves (`back` steps it back) by `delta_ms`,
        /// then `event` happens.
        #[derive(Debug, Clone)]
        struct Step {
            back: bool,
            delta_ms: u64,
            event: Event,
        }

        /// A call and what it did.
        #[derive(Debug)]
        struct Traced {
            now_ms: u64,
            event: Event,
            /// The sender and action of an `Event::Action`.
            action: Option<(String, Action)>,
            before: Option<String>,
            after: Option<String>,
            outputs: Vec<Output>,
        }

        fn sender() -> impl Strategy<Value = &'static str> {
            prop_oneof![
                3 => Just(ALICE),
                3 => Just(BOB),
                3 => Just(POLICY),
                1 => Just("operators/alice/arm"),
                1 => Just("bots/other"),
            ]
        }

        fn delta_ms() -> impl Strategy<Value = u64> {
            let hold = CONFIG.hold_timeout_ms;
            let age = CONFIG.max_action_age_ms;
            prop_oneof![
                Just(0),
                Just(1),
                Just(hold - 1),
                Just(hold),
                Just(hold + 1),
                Just(age),
                Just(age + 1),
                0..1000u64,
            ]
        }

        fn basis() -> impl Strategy<Value = Basis> {
            let age = CONFIG.max_action_age_ms;
            prop_oneof![
                3 => Just(Basis::Back(0)),
                1 => Just(Basis::Back(1)),
                1 => Just(Basis::Back(age - 1)),
                2 => Just(Basis::Back(age)),
                2 => Just(Basis::Back(age + 1)),
                2 => (0..=age).prop_map(Basis::Back),
                1 => Just(Basis::Ahead(1)),
                1 => (1..100u64).prop_map(Basis::Ahead),
                1 => Just(Basis::At(0)),
                1 => Just(Basis::At(u64::MAX)),
            ]
        }

        fn dt_ms() -> impl Strategy<Value = u64> {
            let age = CONFIG.max_action_age_ms;
            prop_oneof![
                Just(0),
                Just(1),
                Just(20),
                Just(age),
                Just(age + 1),
                Just(u64::MAX),
                Just(u64::MAX / 2),
                0..50u64,
            ]
        }

        fn seq() -> impl Strategy<Value = u64> {
            prop_oneof![4 => 0..4u64, 1 => Just(u64::MAX), 1 => any::<u64>()]
        }

        fn event() -> impl Strategy<Value = Event> {
            prop_oneof![
                1 => prop_oneof![1 => Just(THE_DRIVER), 2 => sender()].prop_map(Event::Left),
                6 => (
                    prop_oneof![3 => Just(None), 1 => sender().prop_map(Some)],
                    seq(),
                    basis(),
                    dt_ms(),
                    0..4usize,
                )
                    .prop_map(
                    |(sender, seq, basis, dt_ms, entries)| Event::Action {
                        sender,
                        seq,
                        basis,
                        dt_ms,
                        entries,
                    }
                ),
                1 => prop::bool::weighted(0.7).prop_map(Event::StartEpisode),
                1 => Just(Event::EndEpisode),
                4 => Just(Event::Tick),
                1 => Just(Event::SessionLost),
            ]
        }

        fn steps() -> impl Strategy<Value = (u64, Vec<Step>)> {
            (
                prop_oneof![Just(0), Just(1000), Just(u64::MAX - 2000), Just(u64::MAX),],
                vec(
                    (prop::bool::weighted(0.1), delta_ms(), event()).prop_map(
                        |(back, delta_ms, event)| Step {
                            back,
                            delta_ms,
                            event,
                        },
                    ),
                    0..60,
                ),
            )
        }

        fn run(start_ms: u64, steps: &[Step]) -> Vec<Traced> {
            let mut arbiter = Arbiter::new(CONFIG);
            let mut now_ms = start_ms;
            steps
                .iter()
                .map(|step| {
                    now_ms = if step.back {
                        now_ms.saturating_sub(step.delta_ms)
                    } else {
                        now_ms.saturating_add(step.delta_ms)
                    };
                    let before = arbiter.driver().map(str::to_string);
                    let event = match step.event {
                        Event::Left(THE_DRIVER) => Event::Left(
                            [ALICE, BOB, POLICY]
                                .into_iter()
                                .find(|&candidate| before.as_deref() == Some(candidate))
                                .unwrap_or(ALICE),
                        ),
                        ref event => event.clone(),
                    };
                    let mut action = None;
                    let outputs = match event {
                        Event::Left(sender) => arbiter.left(now_ms, sender),
                        Event::Action {
                            sender,
                            seq,
                            ref basis,
                            dt_ms,
                            entries,
                        } => {
                            let basis = match *basis {
                                Basis::Back(back) => now_ms.saturating_sub(back),
                                Basis::Ahead(ahead) => now_ms.saturating_add(ahead),
                                Basis::At(at) => at,
                            };
                            let chunk = chunk(seq, basis, dt_ms, entries);
                            let sender = sender.or(before.as_deref()).unwrap_or(ALICE);
                            let outputs = arbiter.action(now_ms, sender, &chunk);
                            action = Some((sender.to_string(), chunk));
                            outputs
                        }
                        Event::StartEpisode(policy_enabled) => {
                            arbiter.start_episode(now_ms, policy_enabled)
                        }
                        Event::EndEpisode => arbiter.end_episode(now_ms),
                        Event::Tick => arbiter.tick(now_ms),
                        Event::SessionLost => arbiter.session_lost(now_ms),
                    };
                    Traced {
                        now_ms,
                        event,
                        action,
                        before,
                        after: arbiter.driver().map(str::to_string),
                        outputs,
                    }
                })
                .collect()
        }

        fn applies(outputs: &[Output]) -> Vec<&Applied> {
            outputs
                .iter()
                .filter_map(|output| match output {
                    Output::Apply(applied) => Some(applied),
                    _ => None,
                })
                .collect()
        }

        /// Who should drive: the operator whose action applied while nobody
        /// or the policy drove, until it goes stale or leaves, else the
        /// policy while an episode enabling it is open and it has not left.
        #[derive(Default)]
        struct DriverModel {
            operator: Option<String>,
            policy: bool,
        }

        impl DriverModel {
            fn on(&mut self, traced: &Traced) {
                match traced.event {
                    Event::Action { .. } => {
                        if let Some(applied) = applies(&traced.outputs).first()
                            && is_operator(&applied.driver)
                        {
                            self.operator = Some(applied.driver.clone());
                        }
                    }
                    Event::Tick => {
                        let stale = traced.outputs.iter().any(|output| {
                            matches!(
                                output,
                                Output::Control(Control {
                                    reason: ControlReason::Stale,
                                    ..
                                })
                            )
                        });
                        if stale {
                            self.operator = None;
                        }
                    }
                    Event::Left(sender) if self.operator.as_deref() == Some(sender) => {
                        self.operator = None;
                    }
                    Event::Left(POLICY) | Event::EndEpisode => self.policy = false,
                    Event::StartEpisode(policy_enabled) => self.policy = policy_enabled,
                    Event::SessionLost => *self = Self::default(),
                    _ => {}
                }
            }

            fn driver(&self) -> Option<String> {
                self.operator
                    .clone()
                    .or(self.policy.then(|| POLICY.to_string()))
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig { max_shrink_iters: 2048, ..ProptestConfig::default() })]

            #[test]
            fn an_apply_comes_only_from_a_fresh_action_of_the_driver((start_ms, steps) in steps()) {
                // Arrange
                let mut last_seqs: HashMap<&str, u64> = HashMap::new();

                // Act
                let trace = run(start_ms, &steps);

                // Assert: every fresh action of the driver applies its
                // remaining entries, and nothing else applies
                for traced in &trace {
                    let now_ms = traced.now_ms;
                    let mut expected = Vec::new();
                    if let Some((sender, action)) = &traced.action {
                        let sender = sender.as_str();
                        let may_drive = traced.before.as_deref() == Some(sender)
                            || (is_operator(sender)
                                && traced.before.as_deref().is_none_or(|driver| driver == POLICY));
                        let fresh = may_drive
                            && last_seqs.get(sender).is_none_or(|&last| action.seq > last)
                            && action.basis <= now_ms
                            && now_ms - action.basis <= CONFIG.max_action_age_ms;
                        if fresh {
                            for (index, values) in action.actions.iter().enumerate() {
                                let at_ms = u128::from(action.basis)
                                    + (index as u128 + 1) * u128::from(action.dt_ms);
                                if at_ms >= u128::from(now_ms) && at_ms <= u128::from(u64::MAX) {
                                    expected.push(Applied {
                                        driver: sender.to_string(),
                                        at_ms: at_ms as u64,
                                        values: values.clone(),
                                        loop_latency_ms: now_ms - action.basis,
                                    });
                                }
                            }
                        }
                        if !expected.is_empty() {
                            last_seqs.insert(sender, action.seq);
                        }
                    }
                    match traced.event {
                        Event::Left(sender) => {
                            last_seqs.remove(sender);
                        }
                        Event::SessionLost => last_seqs.clear(),
                        _ => {}
                    }
                    prop_assert_eq!(applies(&traced.outputs), expected.iter().collect::<Vec<_>>(), "{:?}", traced);
                }
            }

            #[test]
            fn the_driver_follows_the_model_and_the_policy_never_overrides_an_operator(
                (start_ms, steps) in steps(),
            ) {
                // Arrange
                let mut model = DriverModel::default();

                // Act
                let trace = run(start_ms, &steps);

                // Assert
                for traced in &trace {
                    if applies(&traced.outputs).iter().any(|applied| applied.driver == POLICY) {
                        prop_assert_eq!(&model.operator, &None, "{:?}", traced);
                    }
                    model.on(traced);
                    prop_assert_eq!(&traced.after, &model.driver(), "{:?}", traced);
                }
            }

            #[test]
            fn control_is_emitted_exactly_when_the_driver_changes_or_goes_stale_and_at_each_episode_start(
                (start_ms, steps) in steps(),
            ) {
                // Act
                let trace = run(start_ms, &steps);

                // Assert
                for traced in &trace {
                    let controls: Vec<&Control> = traced
                        .outputs
                        .iter()
                        .filter_map(|output| match output {
                            Output::Control(control) => Some(control),
                            _ => None,
                        })
                        .collect();
                    let expected = if traced.before != traced.after {
                        let reason = match traced.event {
                            Event::Action { .. } => ControlReason::Action,
                            Event::Tick => ControlReason::Stale,
                            Event::Left(_) => ControlReason::Left,
                            Event::StartEpisode(_) | Event::EndEpisode => ControlReason::Episode,
                            Event::SessionLost => {
                                prop_assert!(controls.is_empty(), "{:?}", traced);
                                continue;
                            }
                        };
                        vec![Control { driver: traced.after.clone(), reason }]
                    } else if matches!(traced.event, Event::StartEpisode(_)) {
                        vec![Control { driver: traced.after.clone(), reason: ControlReason::Episode }]
                    } else if matches!(traced.event, Event::Tick) && !controls.is_empty() {
                        prop_assert!(traced.after.is_some(), "{:?}", traced);
                        vec![Control { driver: traced.after.clone(), reason: ControlReason::Stale }]
                    } else {
                        Vec::new()
                    };
                    prop_assert_eq!(controls, expected.iter().collect::<Vec<_>>(), "{:?}", traced);
                }
            }

            #[test]
            fn a_hold_lasts_until_the_next_apply_and_is_emitted_once((start_ms, steps) in steps()) {
                // Arrange
                let mut hold: Option<HoldReason> = None;

                // Act
                let trace = run(start_ms, &steps);

                // Assert
                for traced in &trace {
                    for output in &traced.outputs {
                        match output {
                            Output::Hold(HoldReason::Stale) => {
                                prop_assert_eq!(hold, None, "{:?}", traced);
                                hold = Some(HoldReason::Stale);
                            }
                            Output::Hold(HoldReason::Session) => {
                                prop_assert_ne!(hold, Some(HoldReason::Session), "{:?}", traced);
                                hold = Some(HoldReason::Session);
                            }
                            Output::Apply(_) => {
                                prop_assert!(matches!(traced.event, Event::Action { .. }), "{:?}", traced);
                                hold = None;
                            }
                            Output::Control(_) => {}
                        }
                    }
                    if traced.after.is_none() && traced.before.is_some() {
                        prop_assert!(hold.is_some(), "nobody drives but no hold: {:?}", traced);
                    }
                    if matches!(traced.event, Event::SessionLost) {
                        prop_assert_eq!(hold, Some(HoldReason::Session), "{:?}", traced);
                    }
                }
            }

            #[test]
            fn the_dead_man_fires_once_hold_timeout_after_the_last_apply_or_driver_change(
                (start_ms, steps) in steps(),
            ) {
                // Arrange
                let mut armed_ms = 0;
                let mut fired = false;

                // Act
                let trace = run(start_ms, &steps);

                // Assert
                for traced in &trace {
                    let stale = traced.outputs.iter().any(|output| {
                        matches!(output, Output::Control(Control { reason: ControlReason::Stale, .. }))
                    });
                    let due = matches!(traced.event, Event::Tick)
                        && traced.before.is_some()
                        && !fired
                        && traced.now_ms.saturating_sub(armed_ms) >= CONFIG.hold_timeout_ms;
                    prop_assert_eq!(stale, due, "{:?}", traced);
                    fired |= stale;
                    if traced.before != traced.after
                        || matches!(traced.event, Event::SessionLost)
                        || !applies(&traced.outputs).is_empty()
                    {
                        armed_ms = traced.now_ms;
                        fired = false;
                    }
                }
            }
        }
    }
}
