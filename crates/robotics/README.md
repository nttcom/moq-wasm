# robotics

Robot teleoperation, cloud policy rollouts, and episode data collection over MoQT using MSF Event Timelines (`draft-ietf-moq-msf-02` §8).

Defines the wire records (`state`, `control`, `episode`, `action`), a pure synchronous `Arbiter` state machine with dead-man hold, the robot runtime (`Robot`), and the driver action writer (`Driver`). Native-only (requires Tokio).

> **Building a robot or cloud policy client?** Start with [`sdk/rust`](../../sdk/rust/README.md#a-robot) (`moq_wasm::robotics::run_robot` and `offer_actions`) for automatic reconnects and simpler setup. Use `robotics` directly to embed the pure `Arbiter` without MoQT or wire custom `moqt`/`broadcast` sessions.

## Room namespace layout

| Namespace | Track(s) | Priority | Owner |
| --- | --- | --- | --- |
| `<room>/robot` | `state`, `control`, `episode` (plus cameras) | `control`: 0, `state`: 32, `episode`: 64 | `Robot` |
| `<room>/operators/<id>` | `action` | `action`: 0 | Human teleoperator |
| `<room>/bots/policy` | `action` | `action`: 0 | Cloud policy (`Driver`) |

Lower priority numbers are delivered first (camera tracks default to `128`).

## Install

```toml
[dependencies]
robotics = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
broadcast = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

## Quickstart: Run the robot control loop

1. Start a local relay (`make relay`, which writes `services/relay/keys/cert.pem`).
2. Inside an `async fn` returning `anyhow::Result<()>`, publish `state` periodically alongside reading commands (drivers echo the latest `state` timestamp as `basis`, so a one-shot state expires after `max_action_age_ms`):

```rust
use std::{sync::Arc, time::Duration};
use broadcast::{Broadcast, RelayConnection};
use moqt::{CertificateVerification, ClientConfig, client::Session};
use robotics::{ArbiterConfig, Command, Robot};
use serde_json::json;

let config = ClientConfig {
    // Use `ClientConfig::default()` for relays with a publicly trusted TLS certificate.
    certificate_verification: CertificateVerification::TrustedCertificates(
        "services/relay/keys/cert.pem".into(),
    ),
    ..Default::default()
};
let session = Arc::new(Session::connect("moqt://127.0.0.1:4433", &config).await?);
let conn = RelayConnection::with_client_session(&session);
let _handler = session.add_handler(Arc::new(conn.clone()));

let room = vec!["anon".into(), "lab".into()];
let broadcast = Broadcast::new(conn, vec!["anon".into(), "lab".into(), "robot".into()]);
broadcast.announce().await?;

let mut robot = Robot::run(session, &broadcast, room, ArbiterConfig::default()).await?;
let _episode_id = robot.start_episode("pick up cup".into(), true).await?;

let mut ticks = tokio::time::interval(Duration::from_millis(50));
loop {
    tokio::select! {
        cmd = robot.next_command() => match cmd {
            Some(Command::Apply { at_ms, values, driver }) => { /* actuate */ }
            Some(Command::Hold(reason)) => { /* hold motors safe */ }
            None => break,
        },
        _ = ticks.tick() => {
            let fields = json!({ "joints": [0.0, 0.0] }).as_object().unwrap().clone();
            robot.publish_state(fields).await?;
        }
    }
}
```

To drive a robot from a policy or operator process, announce `<room>/bots/policy` (or `<room>/operators/<id>`), call `Driver::offer(&broadcast).await?`, and send chunks with `driver.send(basis_ms, dt_ms, actions).await?`.

## Key types

| Item | Role |
| --- | --- |
| `Robot`, `Command` | Watches drivers under `<room>`, runs `Arbiter`, publishes `state`/`control`/`episode`, and yields `Command::Apply` / `Command::Hold` |
| `Driver` | Offers an `action` event-timeline track and sends sequenced action chunks (`send`) |
| `Arbiter`, `ArbiterConfig`, `Output`, `Applied`, `HoldReason` | Pure synchronous arbitration + dead-man state machine (zero I/O) |
| `Record<D>`, `encode_object`, `decode_object` | MSF event timeline JSON envelope `[{"T": <ms>, "data": ...}]` |
| `State`, `Action`, `Control`, `ControlReason`, `Episode`, `TrackKind` | Typed track payloads, canonical track names, priorities, and `catalog_track()` builders |

## Safety & arbitration invariants

- **Operator precedence**: Only `<room>/operators/<id>` (single segment) and `<room>/bots/policy` can drive. Operators always preempt policy and hold control until stale or disconnected; policy drives only during an open episode with `policy_enabled = true`.
- **Clockless freshness**: Action age (`now_ms - action.basis`) uses only the robot's clock. Actions older than `max_action_age_ms` (default `300 ms`), from the future, or with non-increasing `seq` are ignored; chunk entries with `at_ms < now_ms` are skipped.
- **Dead-man hold**: After `hold_timeout_ms` (default `500 ms`) without an applied action, `Arbiter` emits `Hold(HoldReason::Stale)` and writes `control` (`reason: "stale"`).
- **Session loss**: Relay disconnect immediately emits `Hold(HoldReason::Session)` and terminates `next_command()` with `None` (create a new `Robot` after reconnecting).
- **Constraints**: `Robot::run` requires `broadcast.namespace() == <room>/robot` and opens `session.watch_tracks(&room)` (no overlapping `SUBSCRIBE_NAMESPACE` on `session`). `publish_state` rejects maps containing `"seq"`.

## Further reading

- Architecture: [docs/architecture/robotics/architecture.md](../../docs/architecture/robotics/architecture.md)
- Runnable examples: [`examples/rust/moq-robot-sim`](../../examples/rust/moq-robot-sim/README.md) · [`examples/rust/moq-robot-policy`](../../examples/rust/moq-robot-policy/)
