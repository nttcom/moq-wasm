# [XS] Name every session event in the relay's authorization gate

Next: replace `_ => None` in `services/relay/src/modules/auth/request_gate.rs:47` with the `MoqtSessionEvent` variants that reach it.

## Goal

A new `MoqtSessionEvent` does not compile until the gate says whether it needs authorization.
Checked: every variant that reaches the wildcard today acts only on the state of its own session, so nothing passes unauthorized now.
