# robot-teleop

Browser teleoperation UI for driving a robot running [`examples/rust/moq-robot-sim`](../../../rust/moq-robot-sim/README.md) (and optionally competing with [`examples/rust/moq-robot-policy`](../../../rust/moq-robot-policy/README.md)) over MoQT with dead-man hold arbitration.

## Run

Run each command in a separate terminal:

```shell
make relay
cargo run -p moq-robot-sim -- --insecure
make browser
make chrome
```

Open `http://localhost:5173/moq-wasm/examples/robot-teleop/`, keep the same room name (default `anon/robot-teleop`), and click **接続**. Hold **WASD** or **arrow keys** to drive the robot on the 2D pad; releasing stops sending actions so the robot's dead-man hold halts it automatically.

## Tracks

- Operator namespace: `<room>/operators/<id>` — publishes MSF event timeline track `action` (`io.skyway.research.moqt.robotics.action.v1`) at 20 Hz while keys are held.
- Robot namespace: `<room>/robot` — subscribes to `state` (x/y position and robot clock `T`) and `control` (active driver and hold reason).

## E2E

```shell
node tests/browser-e2e/run-robot-teleop-e2e.mjs
```
