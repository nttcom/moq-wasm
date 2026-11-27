# remote-monitoring

Multi-camera live monitoring console and in-browser camera publisher with instant relay-cache review (rewind/step) over MoQT.

## Run

Quickest local demo (starts a relay and three test-pattern camera streams at location `demo`):

```shell
docker compose -f compose.demo.yml --profile monitoring up                                        # terminal 1
make chrome BROWSER_EXAMPLE_PATH='/moq-wasm/examples/remote-monitoring/index.html?location=demo'  # terminal 2
```

Click **接続** to watch all cameras published to `demo`.

Without Docker Compose, run `make relay`, `make browser`, and `make chrome`, then open:

- **Monitor**: `http://localhost:5173/moq-wasm/examples/remote-monitoring/?location=demo`
- **Publisher**: `http://localhost:5173/moq-wasm/examples/remote-monitoring/?mode=publisher&location=demo&cam=cam-1` (publishes a synthetic test pattern, real camera, or video file to `<root>/<location>/<cam>`)

## Controls & Query Parameters

- **Monitor keyboard shortcuts**: `←` / `→` switch cameras in live mode; `↓` or clicking the stage enters review mode (`←` / `→` step 1 s, `↑` returns to live).
- **Query parameters**: `?mode=monitor|publisher`, `?location=<name>`, `?cam=<id>`, `?relay=<url>`.
