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
 robot ◄─── action of <room>/operators/<id> and <room>/bots/policy
```

A room holds one robot. The robot publishes the broadcast `<room>/robot`;
each operator publishes `<room>/operators/<operator_id>`; the policy bot
publishes `<room>/bots/policy`, as bots do (docs/architecture/bot-dispatcher/architecture.md).
The robot subscribes to the namespace `<room>` and to the `action` track of
every broadcast that may drive it.

The relay enforces each token's `publish` path, so the namespace of an
action track identifies its sender: the robot trusts a namespace, not a
field in a record. The app's server issues the robot a token with
`publish: <room>/robot` and `subscribe: <room>`, and each operator one with
`publish: <room>/operators/<operator_id>`.

### Tracks
Every non-media track is an MSF event timeline (draft-ietf-moq-msf-02 §8)
indexed by wallclock (`T`, milliseconds since the Unix epoch on the robot's
clock unless noted). Its `data` is a JSON object whose structure the
track's `eventType` defines (§5.2.6): `io.skyway.research.moqt.robotics.<track>.v1`,
such as `io.skyway.research.moqt.robotics.state.v1`, in the reverse of the
relay's domain as the chat's `io.skyway.research.moqt.chat.v1` is.
Milliseconds are fine enough for control loops up to a few hundred hertz.

| Broadcast | Track | Packaging | Groups | Content |
| --- | --- | --- | --- | --- |
| robot | `camera_<name>` | `loc` | one per keyframe | video, LOC Timestamp in robot wall clock |
| robot | `state` | `eventtimeline` | one per record | joint positions, velocities, gripper, … at each control tick |
| robot | `control` | `eventtimeline` | one per record | the current driver and why it changed |
| robot | `episode` | `eventtimeline` | one per episode | start, task instruction, end, success |
| operator / policy | `action` | `eventtimeline` | one per record | a chunk of actions and the observation it answers |

`state`, `control` and `action` are independent of their history,
so each record is its own group of one object, as §8.4 recommends. A late
subscriber reads the current value with a joining FETCH of the current group
and needs no periodic repeat. Each group travels on a stream of its own, so
a subscriber may receive the groups out of order; the newest record is the
one with the largest Group ID, and a reader that acts on the current value
drops a group older than one it has read. `episode` is a sequence: each
episode is one group whose first object opens it and whose later objects
append to it (§8.4 incremental updates).

The cameras use the existing media source (`add_source` in the SDKs), so
the operator page plays them with `LivePlayer` and the recorder stores them
as it stores any camera.

#### Records
`state`:
```json
{"T": 1763600000123, "data": {"seq": 4512, "joints": [0.12, -0.40, 1.05, 0.0, 0.33, 0.0], "gripper": 0.8}}
```
`data` beyond `seq` is the robot's own schema, and the exporter maps it to
dataset columns by configuration.

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

`control`: `{"T": …, "data": {"driver": "operators/alice", "reason": "action"}}`,
with `driver` relative to the room, `null` when nobody drives, and `reason`
one of `action` (an operator's fresh action took the robot), `left`,
`stale`, `episode`.

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
rule. The rules live in `robotics::Arbiter`, a synchronous state machine
without I/O or a clock of its own: each call passes the robot's time, and
`Robot` feeds it the events it reads from MoQT and acts on what it returns
(apply an action entry, publish `control`, hold). The application gives
`Robot` two durations, `max_action_age` (300 ms by default) and
`hold_timeout` (500 ms by default), and receives the actions to execute and
the holds as commands (see The robot's runtime). The defaults are starting
points, to be tuned on the example over a real network:

- **Candidates.** Operators (`operators/<id>`) take precedence over the
  policy (`bots/policy`); any other namespace never drives. An operator's
  fresh action makes it the driver while nobody or the policy drives; it
  drives until its actions go stale or its PUBLISH_NAMESPACE_DONE, and
  another operator's actions are dropped meanwhile. The policy drives while
  no operator does and an episode is open with the policy enabled, so it
  starts at the episode's start and again when the driving operator goes
  stale or leaves; its PUBLISH_NAMESPACE_DONE stops it until the next
  episode. An operator's action while the policy drives is an
  intervention; the `control` record shows it.
- **Freshness.** An action is applied only when its sender may drive, its
  `seq` is above the last one applied from the same namespace and
  `0 ≤ now − basis ≤ max_action_age`; within a chunk, entries whose time has
  passed are skipped. A chunk with no entry left moved nothing: it counts
  neither as applied for the dead man nor for `seq`, and makes nobody the
  driver. Other actions are dropped. A namespace's last `seq` is forgotten
  with its PUBLISH_NAMESPACE_DONE, so a publisher that returns may start
  over.
- **Dead man.** When the driver has applied no action for `hold_timeout`
  since its last applied one or since it became driver, the robot tells the
  application to hold (hold position, stop motors: the application's
  choice) and publishes `control` with reason `stale`. A stale operator
  stops driving, handing back to the policy during an episode that enables
  it; the policy stays the driver, once stale until its next applied action.
  When nobody drives any more, the robot holds at once. A hold is issued
  once: not again before an action is applied.
- **Session loss.** When the robot's session to the relay ends, it holds at
  once, without waiting for `hold_timeout`, even when it already holds
  because the driver went stale, and forgets the drivers, the episode and
  every `seq`; it publishes no `control`, which could not reach anyone.

`Robot` publishes `state` from the values the application hands it at each
control tick, `control` on each change, and `episode` on the application's
start and end calls. It does not run the control loop; the application
calls it from its own.

#### The robot's runtime
```rust
let robot = Robot::run(session, &broadcast, room, ArbiterConfig::default()).await?;
robot.publish_state(fields).await?;                  // seq and T assigned
let id = robot.start_episode(task, policy_enabled).await?;
robot.end_episode(success).await?;
while let Some(command) = robot.next_command().await { /* Apply { at_ms, values, driver } or Hold(reason) */ }
```

- `broadcast` is the robot's `<room>/robot` broadcast on `session`'s
  connection (`RelayConnection::with_client_session`), which the application
  also feeds its cameras into. `Robot::run` offers `state`, `control` and
  `episode` on it with their priorities and watches the namespaces and
  tracks under `<room>` with one SUBSCRIBE_NAMESPACE
  (`Session::watch_tracks`). A session may hold no other SUBSCRIBE_NAMESPACE
  overlapping `<room>` (NAMESPACE_PREFIX_OVERLAP, draft-14 §9.28), so the
  robot does not subscribe per candidate namespace.
- One task owns the `Arbiter` and feeds it, in one order, the namespace
  events, the candidates' records, the application's episode calls, a tick
  every `hold_timeout / 10` (at least every 50 ms), and the end of the
  session. It turns `Apply` and `Hold` into the commands `next_command`
  returns, and writes `Control` to the `control` track itself.
  `publish_state` writes directly, without that task.
- The relay forwards each PUBLISH under `<room>`. The robot accepts a
  candidate's `action` with the track's Subscriber Priority and DELIVERY
  TIMEOUT `max_action_age`, from the group the PUBLISH found (a Standalone
  FETCH of its Largest Location), and rejects every other track. One reader
  task per accepted PUBLISH reads it until it ends; a PUBLISH of the track
  again replaces it, so a candidate may announce its namespace before its
  track and may end the track and publish it again. The
  PUBLISH_NAMESPACE_DONE drops the candidate's reader and is
  `Arbiter::left`.
- Episode ids count from 1 for each `Robot`; `start_episode` fails while an
  episode is open, `end_episode` while none is.
- When the session ends, `next_command` returns `Hold(Session)` and then
  `None`; the `Robot` is not reused.

### Priority and delivery
Lower values go first (draft-14 §7). Control must not wait behind video:

| Track | Publisher Priority | Subscriber Priority (driver side) | DELIVERY TIMEOUT |
| --- | --- | --- | --- |
| `action`, `control` | 0 | 0 | `max_action_age` |
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

### Recording and datasets
The recorder's room archive (`format: "moq"`) already stores catalogs, LOC
cameras and the event timelines a catalog lists, on one recording clock. It
gains the event timelines of every broadcast in the room, so `state`,
`control`, `episode` and each driver's `action` are archived
next to the cameras. A robot room is recorded by starting the recorder bot
in it, as for a meeting.

The exporter writes a LeRobot dataset (one episode per `episode` group:
the observations at the dataset's fps, aligned by their timestamps, the action applied at each
frame, the task instruction, success, and which driver produced each frame).
It reads the recording through the existing replay (`POST
/recordings/{id}/publish`) with FETCH, so it needs no second archive reader,
and is written in Python on the `lerobot` package, the format's reference
writer. MCAP export, for viewing in Foxglove, follows as a later step.

### Policy bot
The bot type `policy` is a Python bot started by `bot-dispatcher`. It
subscribes to the robot's cameras, `state` and `episode`, pairs each
frame with the newest `state` before it,
runs the model the deployment configures, and publishes `action` chunks with
the `basis` of the observation it used. Which model, and whether inference
runs in the bot or in a separate server, is the deployment's choice; the
example ships a trivial policy.

### SDKs
`robotics` holds the record types, `Robot` and the track setup
helpers. Each SDK wraps it without reimplementing it:

| Side | SDK | Uses |
| --- | --- | --- |
| robot | Rust (`moq_wasm::robotics`), Python | `Robot` |
| operator | browser | record types, action writer |
| policy bot, exporter | Python | record types, action writer |

The Rust SDK adds the module with the same shape as the Python one
(docs/architecture/moq-wasm/sdk.md).

The records are read through `RemoteBroadcast.event_timeline(name)` of each
SDK's media layer, which starts from the record the relay holds (a joining
FETCH) and subscribes again when the track ends. The Python SDK does not
have it yet; it gains it under this name, with the `priority` and
`delivery_timeout` of a SUBSCRIBE, which the table in Priority and delivery
sets per track.

### Telemetry
`Robot` logs each driver change (with `control_reason`), each hold, and each
episode's start and end at INFO, and the loop latency of each applied action
at DEBUG; a metric of it follows with the SDKs. New log fields, added to the
vocabulary in `AGENTS.md` before use: `driver` (the namespace driving,
relative to the room), `hold_reason` (`stale`, `session`), `control_reason`,
`episode_id`, and `loop_latency_ms` (`now − basis` of an applied action).

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
2. `robotics`: record types and track setup helpers, with proptest
   properties for the records.
3. `robotics`: `Robot` (arbitration, dead man, `state` / `control` /
   `episode` publishing).
4. The Rust SDK's `moq_wasm::robotics`; the Python SDK's module follows
   when a Python robot needs it.
5. Example: a simulated robot on the Rust SDK and an operator page in
   `examples/browser` (cameras, keyboard or gamepad actions), with an E2E
   test.
6. `recorder`: archive every broadcast's event timelines in a room.
7. `bot-dispatcher`: the `policy` bot type and an example policy bot.
8. Later: the LeRobot exporter, MCAP export, and a ROS 2 bridge (its own
   design).

## Open questions
- How fast the replay serves FETCH for a long recording, which sets how long
  an export takes.
