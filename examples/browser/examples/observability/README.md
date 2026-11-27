# observability

Interactive relay-mesh topology viewer and time-series chart drawer backed by [`services/observability`](../../../../services/observability/README.md).

## Run

Start a demo relay with ClickStack, HyperDX, and `services/observability`:

```shell
docker compose -f compose.demo.yml -f compose.observability.yml up   # terminal 1
make chrome                                                          # terminal 2
```

Open `http://localhost:5173/moq-wasm/examples/observability/`. On `localhost` / `127.0.0.1` it queries `http://localhost:8095`; otherwise it queries `https://observability.moqt.research.skyway.io` using the current access mode's bearer token (or `anon` in Public mode).

## Features

- **Topology view**: Shows active relays in the center and connected clients on an outer ring. Animated dashed links represent per-hop traffic (speed = bitrate; downlink colored by packet loss). Supports pan, zoom, and double-click reset.
- **Namespace filter & Namespaces panel**: Filter traffic client-side by Track Namespace prefix, or search active namespaces across the selected window.
- **Time range & scrubber**: Inspect live data or scrub through up to 7 days of history (`1m`–`7d`, `←`/`→` for 1 s steps, `Shift` + arrow for 10 s).
- **Chart drawer**: Click **Relays**, a relay, a client, or a link (traces the full pub → sub route) to open per-entity `Network`, `Sessions`, `Media`, `Transport`, and `Streams` charts. Clicking any point on a chart scrubs the topology to that timestamp.
