# tests

Relay E2E scenarios. Each directory holds a client crate and a `run.sh` that
starts the relays with Docker Compose, runs the client and tears the stack
down.

| Script | Checks |
| --- | --- |
| `./tests/auth-e2e/run.sh` | JWT acceptance, expiry and token refresh against two relays and the VTS |
| `./tests/fetch-e2e/run.sh` | FETCH range resolution against the relay cache |
| `./tests/cache-eviction-e2e/run.sh` | Objects leave the cache after the TTL and idle tracks are reclaimed |
| `./tests/cascading-relay-e2e/run.sh` | Publish on `relay-a`, subscribe and fetch through `relay-b` |
| `./tests/multiple-publishers-e2e/run.sh` | A second publisher of the same track is ignored |
| `./tests/object-dedup-e2e/run.sh` | Republishing a group does not duplicate objects for FETCH |
| `./tests/moqtsink-e2e/run.sh` | The GStreamer `moqtsink` publishes the expected tracks |

Browser E2E: the Playwright specs live in `examples/browser/tests`, and the
runners in `browser-e2e/` start the relay, VTS and Vite around them. Run them
through `make browser-e2e-*` or `node tests/browser-e2e/run-*-e2e.mjs`.
