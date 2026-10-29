# tests

Relay E2E scenarios. Each directory holds a client crate and a `run.sh` that
starts the relays with Docker Compose, runs the client and tears the stack
down.

| Script | Checks |
| --- | --- |
| `./tests/auth-e2e/run.sh` | JWT acceptance, expiry and token refresh against two relays and the VTS |
| `./tests/fetch-e2e/run.sh` | FETCH range resolution against the relay cache |
| `./tests/cache-eviction-e2e/run.sh` | Objects leave the cache after the TTL and idle tracks are reclaimed |
| `./tests/cascading-relay-e2e/run.sh` | Publish on `relay-a`, subscribe and fetch through `relay-b`; a second subscriber's joining FETCH returns an object relay-b never cached |
| `./tests/multiple-publishers-e2e/run.sh` | A second publisher of the same track is ignored |
| `./tests/object-dedup-e2e/run.sh` | Republishing a group does not duplicate objects for FETCH |
| `./tests/moqtsink-e2e/run.sh` | The GStreamer `moqtsink` publishes the expected tracks |

Browser E2E: the Playwright specs live in `examples/browser/tests`, and the
runners in `browser-e2e/` start the relay, VTS and Vite around them. Run them
through `make browser-e2e-*` or `node tests/browser-e2e/run-*-e2e.mjs`.

`node tests/browser-e2e/run-console-e2e.mjs` starts the VTS and the console on
one temporary registry file, then drives the console's pages: a token it
issues and one signed with the secret it shows pass the VTS until the secret
is rotated or the app is deleted. With two apps, the sidebar switcher lists
both, each app's pages show only its own appId, a token passes only as the
app whose secret signed it, and rotating or deleting one app leaves the
other's tokens valid. Set `VTS_PORT` when 8081 is taken.

`node tests/browser-e2e/run-ptz-tracking-e2e.mjs` runs the PTZ tracking bot
without a camera: `browser-e2e/fake-onvif-ingest.py` streams a synthetic H.264
picture through `moq-cli publish` as `anon/onvif/client` and records the
bot's `command` track, and the runner's fake System One server answers where the target
is. The specs check that the page shows the step the bot decided, that
onvif-ingest would receive ContinuousMove then Stop, that the step turns the
other way when the target moves, and the System One status while answers are
late. Set `RELAY_PORT`, `VTS_PORT` and `MEDIA_E2E_WEB_PORT` (with
`MEDIA_E2E_MOQT_URL`) when the defaults are taken.
