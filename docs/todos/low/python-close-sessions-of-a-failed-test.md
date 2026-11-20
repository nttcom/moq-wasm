# [S] Close a failed Python binding test's sessions so the relay teardown stays quiet

Next: in `sdk/python/tests/test_media_source.py:420`'s test, move `session.close()` into a `finally` (or a fixture), then do the same for the other relay tests.

## Goal

A relay test that fails gives one failure, not a second `subprocess.TimeoutExpired` ERROR at teardown of the last test (CI run 44204863047).
Cause: the leaked sessions stay on the session-scoped relay; its drain waits `RELAY_DRAIN_TIMEOUT_SECS` (30 s, the GOAWAY has no URI and migrating back to the same relay is refused) plus a 3 s flush, past `stop()`'s 30 s wait (`sdk/python/tests/conftest.py:70`).

## Plan

Fix the test cleanup, not the timeouts: a longer `stop()` wait still hides which test leaked.
