# broadcast

Publishes and reads **MSF broadcasts** (`catalog`, LOC/CMAF media tracks, media timelines, and event timelines) over [`moqt`](../moqt/README.md) using [`mediapack`](../mediapack/README.md) `MediaEvent`s.

Shared by `ingest-gateway`, `egress-gateway`, `sip-gateway`, `recorder`, [`gst-plugin-moqt`](../gst-plugin-moqt/README.md), [`robotics`](../robotics/README.md), and all three SDKs so every publisher works with every player. Compiles on native targets and `wasm32`.

> **Building a standalone Rust app?** Use [`sdk/rust`](../../sdk/rust/README.md) (`moq-wasm`) for automatic reconnects, token refresh, and a simpler broadcast API. Use `broadcast` directly for custom `mediapack` pipelines, gateways, or plugins.

## Install

```toml
[dependencies]
broadcast = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
mediapack = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
```

No Cargo features are required (`broadcast` enables `mediapack/moqt` internally).

## Quickstart: Publish `MediaEvent`s

1. Start a local relay (`make relay`, which writes `services/relay/keys/cert.pem`).
2. Inside an `async fn` returning `anyhow::Result<()>` on a Tokio runtime:

```rust
use std::sync::Arc;
use broadcast::{Broadcast, BroadcastWriter, RelayConnection, SourceTracks};
use moqt::{CertificateVerification, ClientConfig, client::Session};

let config = ClientConfig {
    // Use `ClientConfig::default()` for relays with a publicly trusted TLS certificate.
    certificate_verification: CertificateVerification::TrustedCertificates(
        "services/relay/keys/cert.pem".into(),
    ),
    ..Default::default()
};
let session = Arc::new(Session::connect("moqt://127.0.0.1:4433", &config).await?);
let conn = RelayConnection::with_client_session(&session);
let _handler = session.add_handler(Arc::new(conn.clone()));

let broadcast = Broadcast::new(conn, vec!["anon".into(), "live".into()]);
// Spawns background tasks and announces the namespace automatically.
let mut writer = BroadcastWriter::run(broadcast, SourceTracks::default());

// Push MediaEvent::VideoConfig, AudioConfig, Video, and Audio as they arrive:
// let outcome = writer.push(event)?;
```

To let `RelayConnection` dial its own session lazily instead of sharing an existing `Session`, pass `RelayConnection::new(Some(MoqtTarget { url, auth_token, verify_certificate }))` (set `verify_certificate: false` only for local self-signed relays).

## Quickstart: Read a broadcast

`BroadcastReader::run` takes the underlying `&moqt::Session` (via `session.moqt_session()` on a `moqt::client::Session`):

```rust
use broadcast::{BroadcastReader, selection::TrackChoice};

let mut reader =
    BroadcastReader::run(session.moqt_session(), "anon/live", TrackChoice::default()).await?;
while let Some(event) = reader.next_event().await {
    // Yields aligned MediaEvent::VideoConfig, AudioConfig, Video, and Audio
}
```

## Main entry points

| Type / Module | Role |
| --- | --- |
| `RelayConnection`, `MoqtTarget` | Owns or borrows a `moqt` session and manages the shared `Origin` and catalog refresh |
| `Broadcast` | Namespace-scoped handle to announce, declare tracks, offer event timelines (`offer_chat`, `offer_event_timeline`), and send objects |
| `BroadcastWriter`, `RenditionTrack` | Non-blocking `push(MediaEvent)` publisher for LOC/CMAF tracks, media timelines, and catalog updates; `RenditionTrack` adds ABR renditions |
| `MediaSource`, `MediaRendition` | Pre-encoded frame publisher with `on_demand` media publishing and keyframe request notifications (used by the SDKs) |
| `BroadcastReader`, `CatalogReader`, `selection` | Subscribes to `catalog` and selected LOC tracks (`TrackChoice`), yielding aligned `MediaEvent`s |

## Track layout

| Track | Packaging | Content |
| --- | --- | --- |
| `catalog` | MSF (`draft-ietf-moq-msf-02`) | Track list; `initDataList` holds AAC config and CMAF init segments |
| `video`, `video_<height>p` | LOC (`draft-ietf-moq-loc-04`) | H.264/H.265 Annex-B (in-band parameter sets), VP8, VP9, or AV1 with capture timestamp |
| `audio` | LOC | Raw AAC, G.711 A-law, Opus, or MP3 frames |
| `<track>_cmaf` | CMAF (`draft-ietf-moq-cmsf-01`) | Single-track `moof` + `mdat` fragments (video, and AAC audio only) |
| `<video>_timeline` / `chat` | MSF timelines | Media timeline (`[pts_ms, [group, obj], encoded_at_ms]`) and event timelines |

Customize default track names via `SourceTracks`.

## Key caveats

- **Group alignment**: Video keyframes open new wall-clock-seeded groups; audio joins the latest video group (dropped before the first video keyframe; audio-only opens a group every 1 s).
- **ABR keyframe ordering**: Push the source video sample to `BroadcastWriter::push()` **before** sending it to [`transcode::Transcoder`](../transcode/README.md) so `FollowSource` renditions match the synchronously assigned keyframe group ID.
- **Publishing policy**: `catalog` is always eagerly `PUBLISH`ed at `Forward State 1` (refreshed on change and every 30 s, `Retention::NewestOnly`). LOC media tracks use `Forward State 1` by default, or `Forward State 0` when `on_demand` is enabled (media flows only while subscribed). CMAF siblings wait for incoming `SUBSCRIBE`.
- **Backpressure & retention**: Media tracks keep 60 s for relay `FETCH`. `BroadcastWriter` buffers 2,310 slots (8 reserved for configs); when full, `push()` returns `PushOutcome::Dropped` and drops video until a keyframe fits.
- **Reader alignment**: `BroadcastReader` subscribes video with `NextGroupStart` (`NEW_GROUP_REQUEST = 0`) and audio with `LargestObject`, dropping audio captured before the first admitted video object.

## Further reading

- Architecture: [docs/architecture/broadcast/architecture.md](../../docs/architecture/broadcast/architecture.md)
- Specifications: [`draft-ietf-moq-msf-02.txt`](../../docs/spec/draft-ietf-moq-msf-02.txt), [`draft-ietf-moq-cmsf-01.txt`](../../docs/spec/draft-ietf-moq-cmsf-01.txt)
