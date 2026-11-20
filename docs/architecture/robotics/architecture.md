# Design: robot teleoperation, cloud policies and data collection over MoQT

## Status
Proposed

## Date
2026-11-20

## Scope
A new library crate `crates/robotics` that defines how a robot, its
operators and a policy exchange observations and actions over MoQT, and the
changes it brings to:

- `crates/moqt`, whose `TrackWriter` gains a Publisher Priority per track.
- The SDKs, which wrap `robotics`: the Rust SDK (`moq_wasm::robotics`, see
  docs/architecture/moq-wasm/sdk.md) and the Python SDK for the robot and the
  policy, the browser SDK for the operator.
- `services/recorder`, which archives the robot room's event timeline tracks.
- `services/bot-dispatcher`, which gains the bot type `policy`.
- Two new tools: an exporter writing LeRobot datasets, and later MCAP.

Teleoperation, cloud inference and data collection are one design: an
operator driving the robot produces a demonstration, a policy driving it
produces a rollout, and an operator taking over from a policy produces an
intervention. All three are the same tracks with a different driver.

Out of scope: the policy model itself, the robot's motion control, physical
safety beyond holding still when actions stop, more than one robot per room,
point clouds and other observations that are neither video nor small
records, and a ROS 2 bridge (a later design, see Plan).

## Context
LiveKit Portal (a robotics layer over LiveKit rooms) moves camera streams,
robot state and control actions between a robot and its operators, and
aligns camera frames and state by timestamp so each control tick gets one
observation. This repository already has the pieces underneath:

- Rooms of broadcasts with MSF catalogs, LOC video with wall-clock LOC
  Timestamps, and MSF event timeline tracks (`crates/broadcast`, the SDKs).
- Subscriber Priority and DELIVERY TIMEOUT per subscription, datagrams, and
  joining FETCH for a late subscriber.
- Bots started per room (`bot-dispatcher`), a recorder that archives a
  room's catalogs, LOC media and event timelines on one clock
  (`recorder`, `format: "moq"`), and replays an archive as MSF VOD.

What is missing is a robot-shaped contract on top: which tracks a robot
publishes, what an action record says, who may drive, what the robot does
when actions stop, and how a recording becomes a training dataset.

Robot-side code is often Rust, policy and dataset code is Python, and the
operator runs in a browser. The logic every side must agree on therefore
lives once, in Rust, and the SDKs wrap it.

## Decision

### Topology
```
            ┌──────── <room>/robot ────────┐
 robot ───► │ cameras (LOC)  state  control │ ───► relay ───► operator page
            │ episode                       │          ├────► policy bot   (<room>/bots/policy)
            └───────────────────────────────┘          └────► recorder     (<room>/bots/recorder)
 robot ◄─── action, intent of <room>/operators/<id> and <room>/bots/policy
```

A room holds one robot. The robot publishes the broadcast `<room>/robot`;
each operator publishes `<room>/operators/<operator_id>`; the policy bot
publishes `<room>/bots/policy`, as bots do (docs/architecture/bot-dispatcher/architecture.md).
The robot subscribes to the namespace `<room>` and to the `action` and
`intent` tracks of every broadcast that may drive it.

The relay enforces each token's `publish` path, so the namespace of an
action track identifies its sender: the robot trusts a namespace, not a
field in a record. The app's server issues the robot a token with
`publish: <room>/robot` and `subscribe: <room>`, and each operator one with
`publish: <room>/operators/<operator_id>`.

### Tracks
Every non-media track is an MSF event timeline (draft-ietf-moq-msf-02 §8)
indexed by wallclock (`T`, milliseconds since the Unix epoch on the robot's
clock unless noted). Its `data` is a JSON object whose structure the
track's `eventType` defines (§5.2.6).

| Broadcast | Track | Packaging | Groups | Content |
| --- | --- | --- | --- | --- |
| robot | `camera_<name>` | `loc` | one per keyframe | video, LOC Timestamp in robot wall clock |
| robot | `state` | `eventtimeline` | one per record | joint positions, velocities, gripper, … at each control tick |
| robot | `control` | `eventtimeline` | one per record | the current driver and why it changed |
| robot | `episode` | `eventtimeline` | one per episode | start, task instruction, end, success |
| operator / policy | `action` | `eventtimeline` | one per record | a chunk of actions and the observation it answers |
| operator / policy | `intent` | `eventtimeline` | one per record | take or release control |

`state`, `control`, `action` and `intent` are independent of their history,
so each record is its own group of one object, as §8.4 recommends. A late
subscriber reads the current value with a joining FETCH of the current group
and needs no periodic repeat. `episode` is a sequence: each episode is one
group whose first object opens it and whose later objects append to it
(§8.4 incremental updates).

The cameras use the existing media source (`add_source` in the SDKs), so
the operator page plays them with `LivePlayer` and the recorder stores them
as it stores any camera.

#### Records
`state`:
```json
{"T": 1763600000123, "data": {"seq": 4512, "joints": [0.12, -0.40, 1.05, 0.0, 0.33, 0.0], "gripper": 0.8}}
```
`data` beyond `seq` is the robot's own schema; the catalog names it by
`eventType`, and the exporter maps it to dataset columns by configuration.

`action`:
```json
{"T": 1763600000180, "data": {"seq": 77, "basis": 1763600000123, "dt_ms": 20, "actions": [[0.13, -0.41, 1.04, 0.0, 0.33, 0.0, 0.8]]}}
```
- `basis` is the robot timestamp the sender acted on: the `T` of the newest
  `state` record, or the LOC Timestamp of the newest camera frame it showed
  (converted to milliseconds), whichever is newer.
- `actions` is a chunk: entry `i` is meant for robot time
  `basis + (i + 1) × dt_ms`. Teleoperation sends chunks of one; a policy
  sends its action horizon.
- `T` is the sender's clock and is informational only.

`intent`: `{"T": …, "data": {"take": true}}` or `{"take": false}`.

`control`: `{"T": …, "data": {"driver": "operators/alice", "reason": "intent"}}`,
with `driver` relative to the room, `null` when nobody drives, and `reason`
one of `intent`, `release`, `left`, `stale`, `episode`.

`episode`: the first object `{"T": …, "data": {"event": "start", "id": 12, "task": "put the cup on the tray"}}`,
the last `{"T": …, "data": {"event": "end", "id": 12, "success": true}}`.

### Time without clock synchronisation
Only the robot's clock judges freshness. An action carries `basis`, a
timestamp the robot itself issued, so the robot computes the age of an
action as `now − basis` on its own clock: the whole loop from observation to
arrival, including the sender's think time. Operators and the policy need
no synchronised clock, and the same number is the loop latency the robot
reports.

Camera LOC Timestamps and `state` records are both on the robot's clock, so
a subscriber aligns them without knowing the robot's offset either.

### Driving and holding still
`robotics::Robot` runs on the robot and owns arbitration and the dead-man
rule. The application gives it the namespaces that may drive, their
precedence, and two durations, and receives the actions to execute:

- **Candidates.** Operators (`operators/*`) take precedence over the policy
  (`bots/policy`). An operator drives after its `intent` `take` and until
  `release`, its PUBLISH_NAMESPACE_DONE, or staleness. The policy drives
  while no operator does and an episode is open with the policy enabled.
  An operator's `take` while the policy drives is an intervention; the
  `control` record shows it.
- **Freshness.** An action from the driver is applied only when its `seq`
  is above the last one applied and `now − basis ≤ max_action_age`; within a
  chunk, entries whose time has passed are skipped. Actions from a candidate
  that does not drive are dropped.
- **Dead man.** When no action has been applied for `hold_timeout`, the
  robot calls the application's hold handler (hold position, stop motors:
  the application's choice) and publishes `control` with reason `stale`.
  The driver stays the driver; its next fresh action resumes control.
- **Session loss.** When the robot's session to the relay ends, it holds at
  once, without waiting for `hold_timeout`.

`Robot` publishes `state` from the values the application hands it at each
control tick, `control` on each change, and `episode` on the application's
start and end calls. It does not run the control loop; the application
calls it from its own.

### Priority and delivery
Lower values go first (draft-14 §7). Control must not wait behind video:

| Track | Publisher Priority | Subscriber Priority (driver side) | DELIVERY TIMEOUT |
| --- | --- | --- | --- |
| `action`, `intent`, `control` | 0 | 0 | `max_action_age` |
| `state` | 32 | 32 | `max_action_age` for operators and the policy, none for the recorder |
| `episode` | 64 | 64 | none |
| cameras | 128 | 128 | the player's default |

The DELIVERY TIMEOUT is per subscription, so one `state` track serves both
needs: an operator and the policy drop a record that arrives too late to
act on, while the recorder receives every record. The `robotics` helpers
set these values; the application may override them.

`moqt::TrackWriter` fixes the Publisher Priority at 128 today
(`crates/moqt/src/modules/moqt/domains/track_writer.rs`), so it gains a
priority per track, exposed by `publish_track` in each SDK. The relay must
then let a higher-priority stream open before lower-priority streams that
wait for credit, which it does not do yet
(docs/todos/high/relay-lower-priority-streams-block-higher-priority.md); that
fix is a prerequisite and is handled separately.

Records travel on streams, not datagrams. A datagram is not resent, so the
recorder would lose records lost on the robot's uplink, and a subscriber
could not choose reliability per subscription as it does with DELIVERY
TIMEOUT.

### Observation alignment
`robotics::Aligner` turns timestamped inputs into one observation per tick
for the policy bot and the exporter. At tick time `t` it takes, per input,
the newest item with timestamp `≤ t`, and yields no observation when any
input's newest item is older than `t − max_skew`. Items are opaque to it
(encoded frames, decoded images, state records); decoding stays with the
caller. On the robot, which holds its own data, no alignment is needed.

### Recording and datasets
The recorder's room archive (`format: "moq"`) already stores catalogs, LOC
cameras and the event timelines a catalog lists, on one recording clock. It
gains the event timelines of every broadcast in the room, so `state`,
`control`, `episode` and each driver's `action` and `intent` are archived
next to the cameras. A robot room is recorded by starting the recorder bot
in it, as for a meeting.

The exporter writes a LeRobot dataset (one episode per `episode` group:
the aligned observations at the dataset's fps, the action applied at each
frame, the task instruction, success, and which driver produced each frame).
It reads the recording through the existing replay (`POST
/recordings/{id}/publish`) with FETCH, so it needs no second archive reader,
and is written in Python on the `lerobot` package, the format's reference
writer. MCAP export, for viewing in Foxglove, follows as a later step.

### Policy bot
The bot type `policy` is a Python bot started by `bot-dispatcher`. It
subscribes to the robot's cameras, `state` and `episode`, aligns them,
runs the model the deployment configures, and publishes `action` chunks with
the `basis` of the observation it used. Which model, and whether inference
runs in the bot or in a separate server, is the deployment's choice; the
example ships a trivial policy.

### SDKs
`robotics` holds the record types, `Robot`, `Aligner` and the track setup
helpers. Each SDK wraps it without reimplementing it:

| Side | SDK | Uses |
| --- | --- | --- |
| robot | Rust (`moq_wasm::robotics`), Python | `Robot` |
| operator | browser | record types, action and intent writers |
| policy bot, exporter | Python | `Aligner`, record types, action writer |

The Rust SDK adds the module with the same shape as the Python one.

### Telemetry
`Robot` logs each driver change and each hold at INFO, and the loop latency
of applied actions as a metric. New log fields, added to the vocabulary in
`AGENTS.md` before use: `driver` (the namespace driving, relative to the
room), `hold_reason` (`stale`, `session`), `episode_id`, and
`loop_latency_ms` (`now − basis` of an applied action).

## Alternatives
- **A custom binary packaging instead of JSON event timelines**: smaller
  records at high rates, but needs a packaging value MSF does not define
  (§5.2.4) and its own parser in every SDK and the recorder. A few hundred
  JSON records per second of joint values are well within what the relay
  and SDKs carry; revisit when a robot needs kHz rates or large states.
- **Datagrams for `action` and `state`**: no head-of-line blocking and no
  stream credit, but no per-subscriber reliability, so recordings would
  miss records (see Priority and delivery).
- **Arbitration in the relay or a control service**: one place to decide,
  but the robot is the only party that must stop when the decision cannot
  reach it, so it decides.
- **Synchronising clocks (NTP, PTP) to judge freshness**: needed only if
  senders stamped their own time; `basis` makes the robot's clock enough.
- **Python-only implementation**: faster to start, but robot code is often
  Rust, and the robot, the policy and the exporter must agree on the same
  records and rules.
- **A Rust archive reader for the exporter**: faster than replaying, but a
  second reader of the archive format beside the recorder's replay.

## Plan
Each step is its own pull request, stacked:

0. `relay`: let higher-priority streams open first (separate work,
   prerequisite).
1. `moqt`: Publisher Priority per track in `TrackWriter`, and `priority` in
   `publish_track` of the Python and browser SDKs.
2. `robotics`: record types, track setup helpers and `Aligner`, with
   proptest properties for the alignment and the freshness rule.
3. `robotics`: `Robot` (arbitration, dead man, `state` / `control` /
   `episode` publishing).
4. Python SDK `moq_wasm.robotics`, and the module in the Rust SDK.
5. Example: a simulated robot in Python and an operator page in
   `examples/browser` (cameras, keyboard or gamepad actions, take and
   release), with an E2E test.
6. `recorder`: archive every broadcast's event timelines in a room.
7. The LeRobot exporter.
8. `bot-dispatcher`: the `policy` bot type and an example policy bot.
9. Later: MCAP export, and a ROS 2 bridge (its own design).

## Open questions
- The `eventType` names (§5.2.6 asks for reverse domain notation): which
  domain this repository uses.
- Whether `T` in milliseconds is fine enough for the robots in view, or
  records need a microsecond field beside it.
- How fast the replay serves FETCH for a long recording, which sets how long
  an export takes.
- Default values of `max_action_age` and `hold_timeout`, to be measured on
  the example over a real network (starting point: 300 ms and 500 ms).
