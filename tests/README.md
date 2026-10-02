# tests

Relay E2E scenarios. Each crate is a client driven by the script of the same
name in `scripts/`, which starts the relays with Docker Compose, runs the
client and tears the stack down.

| Script | Checks |
| --- | --- |
| `./scripts/auth-e2e.sh` | JWT acceptance, expiry and token refresh against two relays and the VTS |
| `./scripts/fetch-e2e.sh` | FETCH range resolution against the relay cache |
| `./scripts/cache-eviction-e2e.sh` | Objects leave the cache after the TTL and idle tracks are reclaimed |
| `./scripts/cascading-relay-e2e.sh` | Publish on `relay-a`, subscribe and fetch through `relay-b` |
| `./scripts/multiple-publishers-e2e.sh` | A second publisher of the same track is ignored |
| `./scripts/object-dedup-e2e.sh` | Republishing a group does not duplicate objects for FETCH |
| `./scripts/moqtsink-e2e.sh` | The GStreamer `moqtsink` publishes the expected tracks |

Browser E2E lives in `examples/browser/tests` and runs through `make browser-e2e-*`
or `node scripts/run-*-e2e.mjs`.
