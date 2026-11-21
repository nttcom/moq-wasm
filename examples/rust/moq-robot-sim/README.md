# moq-robot-sim

A simulated robot for the robot teleoperation example
(`docs/architecture/robotics/architecture.md`), on the Rust SDK's
`moq_wasm::robotics`. The robot is a point on the square [-1, 1]² that
moves at the velocity `[vx, vy]` of the latest applied action and stops
when the robot holds. It publishes its position as `state` every 50 ms.

```sh
cargo run -p relay
cargo run -p moq-robot-sim -- --insecure --room anon/robot-teleop
```

Then open the operator page, `examples/browser/examples/robot-teleop`, with
the same room, and drive with the arrow keys or WASD. Releasing the keys
stops the actions: after the hold timeout the robot holds and the operator
stops driving.

With `--policy-episode <task>` it opens an episode in which the policy
(`examples/rust/moq-robot-policy`) drives whenever no operator does; an
operator's keys intervene, and the policy takes over again once they are
released.

It applies each action entry as it arrives, without waiting for the entry's
time, and has no camera.
