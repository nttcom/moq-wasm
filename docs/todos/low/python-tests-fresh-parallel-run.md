# [S] Make the first parallel Python SDK run after a rebuild pass

Next: build `relay`, `vts`, and `auth-token` once before the xdist workers start, not in each worker's `relay_auth` (`sdk/python/tests/conftest.py:135`).

## Goal

`cd sdk/python && uv run pytest -n auto` passes on its first run after Rust sources change and with no `services/relay/keys/`.
Today 62 tests errored with `auth-token ... died with SIGKILL` while workers rebuilt it (suspected: macOS kills a binary replaced while it runs), and `silent_relay` (`sdk/python/tests/test_session.py:311`) hit `FileNotFoundError` reading `keys/cert.pem` before any relay wrote it.
