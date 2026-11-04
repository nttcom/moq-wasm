# [S] Release the origin lock while PUBLISH waits for PUBLISH_OK

Next: in `crates/moqt/src/modules/moqt/domains/origin.rs:214`, drop `state` before `publisher.publish(...).await` at `origin.rs:228` and lock again to push the outlet.

## Goal

A PUBLISH the relay does not answer holds `Origin::state` for up to 10 s (`CONTROL_MESSAGE_RESPONSE_TIMEOUT`), so `Origin::write` and every other `publish` on the origin wait with it. After the fix, a test with an unanswered PUBLISH shows another track's `write` completing before the timeout.

## Plan

Open: two concurrent `publish` calls of the same track on one session may both send PUBLISH once the lock is dropped; recheck `outlets` after relocking, or mark the request in flight.
