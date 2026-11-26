# [S] Keep a Python test relay from losing its port to another test worker

Next: hold the port that `free_relay_port` (`sdk/python/tests/conftest.py:42`) picks until the relay binds it, or start the relay again on another port when it exits with `failed to open the transport endpoint`.

## Goal

`pytest` passes when xdist workers start relays at the same time.
On 2026-11-26 `pytest (lowest)` ended with 47 setup errors, all in worker `gw2`: its shared relay logged `failed to open the transport endpoint on port 55711` and exited, while `pytest (latest)` passed on the same commit.
`free_relay_port` closes its probe sockets before the relay binds, so another worker can take the port in between.
