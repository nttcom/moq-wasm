# [S] Treat a PUBLISH_DONE whose request the subscriber already withdrew as sent

Next: in the draft-18 `Notification::PublishDone` arm (`crates/moqt/src/modules/moqt/version/draft18.rs:978`), return `Ok(())` when no request stream is left, as `notify_end` (`version.rs:1055`) does for a lost connection; then drop the relay's call-site case (`services/relay/src/modules/data_plane/egress/group_sender.rs:1056`).

## Goal

Ending a track that a subscriber unsubscribes from at the same moment succeeds. Today, on draft-18, `finish_incoming` (`draft18/request_streams.rs:780`) takes the request stream as soon as the UNSUBSCRIBE arrives, but `Origin::release` (`domains/origin.rs:552`) drops the outlet only later, when the session event is handled. An `Origin::end_track` in between sends PUBLISH_DONE, finds no stream, and fails with `closed stream`.
Seen twice: the media E2E's relay logged `ERROR failed to send PUBLISH_DONE error=closed stream` (1 run in 4). The Python SDK's `LocalBroadcast.__aexit__` raised `RuntimeError: closed stream` from `TrackWriter.close` (`sdk/python/python/moq_wasm/_session.py:300`) in `test_pipecat.py`'s `test_a_participant_switching_its_tracks_again_and_again_...` (2 runs in 30). Its relay log shows the relay's UNSUBSCRIBEs to alice and her PUBLISH_NAMESPACE_DONE within 1 ms of each other.

## Plan

Draft-14 sends PUBLISH_DONE on the control stream, so only draft-18 fails. The subscription has already ended, so returning `Ok` loses nothing.
Do not catch the error in the SDK or the tests: every publisher would need the same workaround.
