# [S] Keep the failed-refresh retry test off the wall clock

Next: in `refresh`'s failing branch (`sdk/python/tests/test_sdk_session.py:209`) add `asyncio.get_running_loop().call_later(0.1, time.sleep, 0.2)`; the test then fails every run, and the fix must make it pass.

## Goal

`test_a_failed_refresh_is_retried_until_the_token_actually_expires` failed once in a full `UV_PYTHON=3.14 uv run pytest` (746 tests); 30/30 alone. Cause, confirmed with the stall above: the 0.6 s token leaves the retry ~0.15 s of real time (`_token.py:118` sleeps half the time left).
An event-loop stall over ~0.15 s inside that window makes the retry find the token expired (`_token.py:99`, `:114`): the session ends with `TokenExpired`, `refreshed` stays `[]`, and `eventually` (`:216`) raises `TimeoutError` after 5 s. The SDK is right to end it, so this is a test bug.

## Plan

Fix: let the test move the keeper's time (a clock `TokenKeeper` reads instead of `time.time`/`loop.time`) rather than lengthen the token, which only shrinks the odds.
The other tests in this file with 0.2–0.6 s tokens (`:140` to `:566`) share the shape; move them in the same change.
