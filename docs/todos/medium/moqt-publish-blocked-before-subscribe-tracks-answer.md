# [S] Send no PUBLISH_BLOCKED before the answer to a SUBSCRIBE_TRACKS

Next: in the draft-18 tests of `crates/moqt/src/modules/moqt/version/draft18.rs`, publish a track under an unanswered SUBSCRIBE_TRACKS with no bidirectional stream left (`use_up_bidi_streams`), and read the first message of its response.

## Goal

The first message on the response of a SUBSCRIBE_TRACKS is REQUEST_OK or REQUEST_ERROR.
Unverified, from reading: `publish_blocked_for` (`crates/moqt/src/modules/moqt/version/draft18.rs:630`) picks a SUBSCRIBE_TRACKS stream whether or not it was answered, so a PUBLISH_BLOCKED can be written first.

## Plan

A NAMESPACE for an unanswered SUBSCRIBE_NAMESPACE is held until its REQUEST_OK (`held_namespaces` in the same file); open: hold the PUBLISH_BLOCKED the same way, or let the PUBLISH wait for a stream.
