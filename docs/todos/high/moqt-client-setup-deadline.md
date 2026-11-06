# [S] Bound the server's wait for the control stream and CLIENT_SETUP

Next: wrap `version::accept` in `accept_new_connection` (`crates/moqt/src/modules/moqt/domains/session_creator.rs:367`) in `executor::timeout`, as `explain_rejected_setup` in the same file does.

## Goal

A peer that finishes the transport handshake but never opens the control stream, or never sends CLIENT_SETUP, is closed after a deadline. Today `draft14::setup::accept` (`crates/moqt/src/modules/moqt/version/draft14/setup.rs:163`) awaits `accept_bi` and then CLIENT_SETUP with no deadline, and the server's keep-alive (`crates/moqt/src/modules/transport/server_transport_config.rs:16`) keeps QUIC's idle timeout from firing, so the `Accepting` task and its connection live forever.

## Plan

`TRANSPORT_HANDSHAKE_TIMEOUT` (`session_creator.rs:27`) bounds only the transport handshake before this wait. Close the transport with a termination code when the deadline passes.
