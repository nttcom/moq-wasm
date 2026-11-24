# [S] Test the setup deadlines without pausing tokio's clock over QUIC

Next: in `crates/moqt/src/modules/moqt/domains/session_creator.rs:938` and `:974`, replace `tokio::time::pause()` with a deadline the test can drive, then run each test 600 times.

## Goal

The CLIENT_SETUP and SERVER_SETUP deadline tests (`:919`, `:961`) pass without a paused clock over real QUIC. They passed 600/600; no failure was reproduced.
Risk: `:919` waits while paused for a CONNECTION_CLOSE to cross loopback UDP. In `:961`, SERVER_SETUP_TIMEOUT (30 s) equals quinn's idle timeout, so idle may fire first if no keep-alive arrives while paused.

## Plan

In-memory QMux does not fit: both tests check one side against a QUIC peer that stops answering.
