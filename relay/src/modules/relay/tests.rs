//! In-process integration tests for the relay data plane, exercising real
//! components across module boundaries (unit tests live next to the module
//! they cover; QUIC-level tests live in `relay/tests/`).

mod harness;
mod pipeline;
