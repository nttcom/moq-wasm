# tests

Relay E2E scenarios. Each directory holds a client crate and a `run.sh` that
starts the relays with Docker Compose, runs the client and tears the stack
down.

| Script | Checks |
| --- | --- |
| `./tests/auth-e2e/run.sh` | JWT acceptance, expiry and token refresh against two relays and the VTS |
| `./tests/fetch-e2e/run.sh` | FETCH range resolution against the relay cache |
| `./tests/cache-eviction-e2e/run.sh` | Objects leave the cache after the TTL and idle tracks are reclaimed |
| `./tests/cascading-relay-e2e/run.sh` | Publish on `relay-a`, subscribe and fetch through `relay-b`; runs on draft-18, then on draft-14 |
| `./tests/multiple-publishers-e2e/run.sh` | A second publisher of the same track is ignored |
| `./tests/object-dedup-e2e/run.sh` | Republishing a group does not duplicate objects for FETCH |
| `./tests/cross-version-e2e/run.sh` | A relay accepting draft-14 and draft-18 relays tracks both ways; starts a local VTS and relay (no Docker) on `CROSS_VERSION_E2E_VTS_PORT` (8081) and `CROSS_VERSION_E2E_RELAY_PORT` (4633) |
| `./tests/moqtsink-e2e/run.sh` | The GStreamer `moqtsink` publishes the expected tracks |
| `./tests/live-translate-e2e/run.sh` | live-translate lists the languages in its catalog and stamps translated audio with the source time |

## Browser E2E

The Playwright specs live in `examples/browser/tests`, and the runners in
`browser-e2e/` start the relay, VTS and Vite around them. Run them through
`make browser-e2e-*` or `node tests/browser-e2e/run-*-e2e.mjs`.

| Runner | Also starts | Checks |
| --- | --- | --- |
| `run-console-e2e.mjs` | The VTS and the console on a temporary registry file | Tokens the console issues pass the VTS until the secret is rotated or the app deleted, without affecting other apps |
| `run-live-translate-e2e.mjs` | live-translate in `--echo` mode | A viewer switches to a translation in sync with the picture and back to the original |
| `run-ptz-tracking-e2e.mjs` | A fake ONVIF camera (`fake-onvif-ingest.py`) and a fake System One server | The PTZ tracking bot sends ContinuousMove then Stop for the boxes it receives |

Set `RELAY_PORT`, `VTS_PORT` or `MEDIA_E2E_WEB_PORT` when a default port is
taken.
