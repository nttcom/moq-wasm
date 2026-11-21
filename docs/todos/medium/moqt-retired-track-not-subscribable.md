# [M] Refuse a new SUBSCRIBE to a track its publisher ended until it is published again

Next: keep a live/ended state per track in `crates/moqt/src/modules/moqt/domains/origin.rs` beside the retained numbering, set it in `end_track` (`origin.rs:709`), and answer a SUBSCRIBE for an ended track as `accept_subscribe` (`origin.rs:588`) answers an unknown one.

## Goal

After a Python `MediaBroadcast.close()` and a text-only broadcast on the same namespace, a fresh SUBSCRIBE to the old `audio` or `catalog` gets TRACK_DOES_NOT_EXIST; today it is accepted (Python SDK review, R3-3).
