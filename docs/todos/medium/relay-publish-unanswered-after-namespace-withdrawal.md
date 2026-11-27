# [S] Reproduce a draft-18 relay leaving a PUBLISH unanswered after the namespace was withdrawn

Next: rerun the Python SDK review R5-6 order (`MediaBroadcast` publishing, `Session.unannounce` while its PUBLISH_NAMESPACE and PUBLISHes are in flight, then `close`) against a draft-18 relay with `RUST_LOG=debug`, and read the relay's `PUBLISH received` / `PUBLISH handled` lines for the PUBLISH left without either.

## Goal

The relay answers every PUBLISH with PUBLISH_OK or REQUEST_ERROR, with a test for the order that left one unanswered.
Not reproduced on 2026-11-27: a relay test (40 rounds; with and without SUBSCRIBE_NAMESPACE, Forward 0, a PUBLISH after the withdrawal) and the Python order above (15 rounds) got every PUBLISH answered; every path of `services/relay/src/modules/control_plane/sequences/publish.rs` answers too.

## Plan

Clients no longer hang on it: `MediaSource::end` and `Origin::end_track` withdraw an unanswered PUBLISH.
