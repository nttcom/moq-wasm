# moq-robot-policy

A trivial policy for the robot teleoperation example
(`docs/architecture/robotics/architecture.md`, Policy bot): it publishes
`<room>/bots/policy` with the Rust SDK's `moq_wasm::robotics::offer_actions`
and answers each `state` of the simulated robot with a velocity toward a
target point.

```sh
cargo run -p relay
cargo run -p moq-robot-sim -- --insecure --room anon/robot-teleop --policy-episode "go to the corner"
cargo run -p moq-robot-policy -- --insecure --room anon/robot-teleop --target 0.5,0.5
```

The operator page, `examples/browser/examples/robot-teleop`, shows
`bots/policy` driving; holding a key there intervenes.
