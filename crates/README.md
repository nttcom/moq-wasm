# Workspace crates

Reusable Rust libraries and plugins under `crates/` (for runnable services see `services/`; for application SDKs see `sdk/`).

## Protocol & formats

| Crate | Purpose |
| --- | --- |
| [`moqt`](moqt/README.md) | Core MoQT transport over QUIC, WebTransport, and browser WebTransport/WebSocket (draft-18 / draft-14) |
| [`msf`](msf/README.md) | MSF catalog (`draft-ietf-moq-msf-02`) and CMAF profile (`draft-ietf-moq-cmsf-01`) types and validation |

## Media & plugins

| Crate | Purpose |
| --- | --- |
| [`mediapack`](mediapack/README.md) | Pure-Rust demuxers, muxers, and bitstream helpers (MPEG-TS, FLV, fMP4, LOC, RTP) |
| [`broadcast`](broadcast/README.md) | Publishes and reads MSF broadcasts (`catalog` + LOC/CMAF tracks + timelines) over `moqt` |
| [`transcode`](transcode/README.md) | In-process GStreamer H.264 ABR ladder transcoder and offline MP4 compositor |
| [`gst-plugin-moqt`](gst-plugin-moqt/README.md) | GStreamer `moqtsink` plugin for publishing H.264/AAC pipelines to a MoQT relay |

## Auth & storage

| Crate | Purpose |
| --- | --- |
| [`auth-token`](auth-token/README.md) | HS256 JWT signing, verification, claim/namespace authorization, VTS client, and token-minting CLI |
| [`app-registry`](app-registry/README.md) | JSON app registry store backed by Cloud Storage or a local file with optimistic concurrency |

## Internal service helpers

Shared workspace crates without standalone READMEs: `google-cloud` (metadata tokens and conditional Cloud Storage calls), `http-server` (HTTP/1 accept loop and JSON responses), `job-launcher` (Cloud Run and local job execution), `robotics` (robot-room arbiter and action/state tracks), `telemetry`, `telemetry-otlp`, and `telemetry-report` (tracing/OTLP export and client telemetry records).

To inspect a crate's direct dependencies, run `cargo tree -p <crate> --depth 1 -e normal`. See the [component list](../AGENTS.md#2-project-scope) for the full workspace layout.
