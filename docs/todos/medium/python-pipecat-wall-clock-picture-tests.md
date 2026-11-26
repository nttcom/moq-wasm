# [S] Run the slow-encode picture tests on a fake clock

Next: replace `time.sleep` in `slow_encodes` (`sdk/python/tests/test_pipecat.py:6115`) with a clock the test advances.

## Goal

`test_synchronized_pictures_slow_to_encode_do_not_hold_up_the_speech` and `test_synchronized_pictures_past_two_seconds_of_them_waiting_are_dropped` pass under any runner load.
They failed on the macOS arm64 wheel job of `python-sdk-v0.1.0` (`19 == 20`, `20 <= 19`) while Linux passed: 20 real 100 ms encodes under pytest-xdist overran `LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS` (3 s, `sdk/python/python/moq_wasm/pipecat/_transport.py:59`).

## Plan

Drive the encode delay and the drain deadline from one virtual-time loop, as the short-token tests do (#1985); widening the margin only moves the edge.
