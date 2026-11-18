# Workspace crates

| Layer | Examples | Role |
| --- | --- | --- |
| Protocol and formats | `moqt`, `msf` | MoQT sessions, MSF catalogs and timelines |
| Media | `mediapack`, `broadcast`, `transcode` | Demux, publish and re-encode media |
| Bindings and plugins | `wasm`, `python`, `gst-plugin-moqt` | Browser, Python and GStreamer APIs |
| Services | `relay`, `live-ingest`, `recorder` | Route, ingest and record streams |
| Shared libraries | `auth-token`, `app-registry`, `telemetry` | Authentication, app storage and diagnostics |

For example, `live-ingest` uses `mediapack` to demux an SRT stream and
`broadcast` to publish its catalog and media tracks. A browser uses the
`wasm` bindings to subscribe through the relay.

For the direct dependencies of a crate, run `cargo tree -p python --depth 1 -e normal`
with its package name. See the [component list](../AGENTS.md#2-project-scope) for the full workspace.
