# moq-wasm (Rust)

Async Rust SDK for Media over QUIC Transport (MoQT) relays, matching the [JavaScript](https://github.com/nttcom/moq-wasm/blob/master/sdk/js/js/README.md) and [Python](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/README.md) SDKs. Clients offer draft-18 first, then draft-14.

## What's included

| Module               | Use for                                                                               |
| -------------------- | ------------------------------------------------------------------------------------- |
| `moq_wasm::*`        | `connect`, `Session` (derefs to `moqt::client::Session`), raw tracks, token refresh   |
| `moq_wasm::media`    | MSF broadcasts (`create_broadcast`, `watch_broadcasts`), `MediaSource`, `EventTimeline` |
| `moq_wasm::robotics` | Robot-room arbiter and action/state tracks (`run_robot`, `offer_actions`)             |
| `moq_wasm::telemetry` | A `tracing` layer that sends logs and spans to client telemetry                      |

## Install

```toml
[dependencies]
moq-wasm = { git = "https://github.com/nttcom/moq-wasm", tag = "rust-sdk-v0.1.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
anyhow = "1"
serde_json = "1" # needed for the media broadcast example
```

Requires `tokio` and a C compiler (for `aws-lc-sys`; cross-compilation requires a target C toolchain). Tested on Linux x86_64/aarch64 and macOS arm64 with Rust 1.100.

## Quickstart: Send and receive a message

Start a local relay with `make relay`, then run this example passing `services/relay/keys/cert.pem` — it prints `hello` (subscribe before writing because live subscriptions receive objects published afterwards):

```rust,no_run
use moq_wasm::{CertificateVerification, ConnectOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cert = std::env::args_os().nth(1).expect("pass the relay certificate path");
    let options = ConnectOptions {
        certificate_verification: CertificateVerification::TrustedCertificates(cert.into()),
        ..Default::default()
    };
    let url = "moqt://127.0.0.1:4433";
    let publisher = moq_wasm::connect(url, options.clone()).await?;
    let subscriber = moq_wasm::connect(url, options).await?;
    let namespace = ["anon", "room"];

    let writer = publisher.publish_track(&namespace, "chat", Default::default()).await?;
    publisher.announce(&namespace).await?;
    let messages = subscriber.subscribe(&namespace, "chat", Default::default()).await?;
    writer.write_group("hello".into()).await?;
    let message = messages.next().await?.expect("track ended before the message");
    println!("{}", std::str::from_utf8(&message.payload)?);

    subscriber.close().await;
    publisher.close().await;
    Ok(())
}
```

Use `moqt://` for raw QUIC or `https://` for WebTransport.

## Publish a media broadcast

Register any static tracks or event timelines on the `BroadcastBuilder`, call `announce()`, then add a `MediaSource` and push pre-encoded frames (H.264, H.265, VP8, VP9, AV1, AAC, Opus). The SDK publishes LOC tracks and an MSF catalog readable by the browser and Python SDKs (it does not encode or decode):

```rust,no_run
use moq_wasm::{Retention, media::{self, MediaSourceOptions}};
use serde_json::json;
use std::time::Duration;

# async fn run(session: &moq_wasm::Session, access_unit: Vec<u8>, timestamp_micros: u64) -> anyhow::Result<()> {
let builder = media::create_broadcast(session, &["anon", "room", "robot-1"]);
let events = builder
    .event_timeline("events", Retention::For(Duration::from_secs(3600)))
    .await?;
let broadcast = builder.announce().await?;

let camera = broadcast.add_media_source(MediaSourceOptions::new("camera"));
camera.configure_video("avc1.42e01f")?;
camera.push_video(access_unit, timestamp_micros)?; // one Annex B access unit; keyframes open a new group
events.append(&json!({"T": 0, "data": "started"})).await?;

broadcast.close().await?;
# Ok(())
# }
```

To publish an H.264/H.265 Annex B byte stream (e.g. an encoder's stdout), cut it into access units with `media::AnnexBSplitter` (`push` each chunk read, `finish` at the end).

## Watch and follow broadcasts

`media::watch_broadcasts` spawns a handler for each remote broadcast under a prefix (skipping this session's own) and aborts it on withdrawal. `RemoteBroadcast` reads the MSF catalog, follows tracks across restarts, and reads retained event timelines:

```rust,no_run
use moq_wasm::media::{self, EventTimelineOptions, FollowOptions, TrackQuery, WatchOptions};

# async fn run(session: &moq_wasm::Session) -> anyhow::Result<()> {
media::watch_broadcasts(session, &["anon", "room"], WatchOptions::default(), |remote| async move {
    let mut catalogs = remote.catalogs().await?;
    if let Some(catalog) = catalogs.next().await? {
        let audio = catalog.select(&TrackQuery { role: Some("audio"), ..Default::default() });
        println!("{:?} offers {:?}", remote.namespace(), audio.map(|track| &track.name));
    }
    let mut events = remote.event_timeline("events", EventTimelineOptions::default());
    let mut status = remote.follow("status", FollowOptions::default()); // optional track if the publisher adds one
    loop {
        tokio::select! {
            record = events.next() => println!("event {record}"),
            object = status.next() => println!("status {} bytes", object.payload.len()),
        }
    }
})
.await?;
# Ok(())
# }
```

See [`examples/rust/moq-broadcast`](https://github.com/nttcom/moq-wasm/blob/master/examples/rust/moq-broadcast/README.md) for a complete runnable publisher and watcher.

## A robot

`robotics::run_robot` runs the robot side of a [robot room](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/robotics/architecture.md) on `<room>/robot`: it publishes `state`, `control`, and `episode`, reads `action` from operators (`<room>/operators/<id>`) and policy (`<room>/bots/policy`), and yields one command at a time (holding when nobody drives, actions stop, or the session ends):

```rust,no_run
use moq_wasm::{media, robotics::{self, ArbiterConfig, Command}};

# fn apply(_: u64, _: &[f64]) {}
# fn hold() {}
# async fn run(session: &moq_wasm::Session) -> anyhow::Result<()> {
let broadcast = media::create_broadcast(session, &["anon", "lab", "robot"])
    .announce()
    .await?;
let mut robot = robotics::run_robot(session, &broadcast, ArbiterConfig::default()).await?;
let episode = robot.start_episode("put the cup on the tray".to_string(), false).await?;
while let Some(command) = robot.next_command().await {
    match command {
        Command::Apply { at_ms, values, .. } => apply(at_ms, &values),
        Command::Hold(_) => hold(),
    }
}
# let _ = episode;
# Ok(())
# }
```

## Authentication & session lifecycle

Pass a JWT in `ConnectOptions::token`. With `refresh_token`, the SDK requests a fresh token 5 minutes before `exp` (retrying every 30 s on error):

```rust,no_run
use moq_wasm::{ConnectOptions, RefreshToken, SessionEnd};

async fn later_token(current: String) -> anyhow::Result<String> {
    // Request a fresh JWT with a later `exp` for the same app from your app server.
    // Returning an unchanged token near `exp` will retry immediately; returning Err backs off for 30 s.
    let _ = current;
    anyhow::bail!("replace with a call to your token endpoint")
}

# async fn work(_: &moq_wasm::Session) -> anyhow::Result<()> { Ok(()) }
# async fn run(jwt: String) -> anyhow::Result<()> {
let session = moq_wasm::connect(
    "https://relay.example:4433",
    ConnectOptions {
        token: Some(jwt),
        refresh_token: Some(RefreshToken::new(later_token)),
        ..Default::default()
    },
)
.await?;

tokio::select! {
    result = work(&session) => result?,
    end = session.closed() => match end {
        SessionEnd::ConnectionLost(close) => eprintln!("relay lost: {close}"),
        SessionEnd::TokenExpired => eprintln!("token expired"),
        _ => {}
    },
}
# Ok(())
# }
```

The session follows `GOAWAY` and reconnects for up to 60 s automatically, restoring its namespaces and subscriptions. `session.closed()` resolves on unrecoverable connection loss, token expiry, or `session.close()`.

## Logging & client telemetry

The SDK logs through [`tracing`](https://docs.rs/tracing); install your own subscriber (e.g. `tracing-subscriber`). To send the logs and spans to an observability server's `/api/client-telemetry`, as the JavaScript and Python SDKs' `telemetry` option does, add `moq_wasm::telemetry::layer` to it before connecting. Records under a session (its own tasks, and futures passed to `session.scoped`) carry the session's latest token, so the server files them under its app; records outside any session are sent without a token:

```rust,no_run
use moq_wasm::telemetry::{self, TelemetryOptions};
use tracing_subscriber::{Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt};

# async fn work(_: &moq_wasm::Session) -> anyhow::Result<()> { Ok(()) }
# async fn run(options: moq_wasm::ConnectOptions) -> anyhow::Result<()> {
let (telemetry_layer, telemetry) = telemetry::layer(TelemetryOptions {
    url: "https://observability.example".to_string(),
    page: "my-robot".to_string(),
});
tracing_subscriber::registry()
    .with(telemetry_layer.with_filter(LevelFilter::INFO))
    .init();

let session = moq_wasm::connect("https://relay.example:4433", options).await?;
let result = session.scoped(work(&session)).await;
telemetry.flush().await; // reports go every 10 s; send the rest before exiting
result
# }
```

## Design

- Architecture & internals: [docs/architecture/moq-wasm/sdk.md](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/moq-wasm/sdk.md).
