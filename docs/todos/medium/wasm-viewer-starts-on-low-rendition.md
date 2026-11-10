# [M] Find why the tile size E2E fails when a viewer starts on the low rendition

Next: repeat `Auto follows the size of a member tile when the view changes` (`examples/browser/tests/meeting-e2e.spec.ts:462`) 500 times in CI with the `LivePlayer` video switch and the received audio levels logged, and read a run that fails at line 484 or 485.

## Goal

The test passes when a member's first catalog lists `camera_low` alone. Of 2,200 runs, 12 failed this way (3 of the 1,100 without the fix of the track definition kept by a subscription, so that fix does not cause them): bob's video stayed 320 wide for 30 s (line 484), or carol's speaking ring never showed (line 485).

## Plan

The publisher lists a rendition once its first chunk is encoded, so a viewer can see `camera_low` before `camera`; it then starts low and steps up after the 13 s of `SETTLE_MS` and `INITIAL_UP_HOLD_MS`.

- Line 484: Auto chose `camera_low -> camera` and subscribed to `camera`, but the switch never completed in the remaining 17 s. Find what the switch waits for.
- Line 485: alice subscribed to carol's audio, yet no audio level reached the ring. Find whether the objects did not arrive or arrived without voice activity.
