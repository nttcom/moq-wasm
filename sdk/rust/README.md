# moq-wasm (Rust)

The Rust SDK of this repository: connect to a MoQT relay, publish and
subscribe to tracks and watch namespaces, in the same shapes as the
[browser SDK](../js/js/README.md) and the [Python SDK](../python/README.md).
Clients offer MoQT draft-18 first, then draft-14. The design is in
[docs/architecture/moq-wasm/sdk.md](../../docs/architecture/moq-wasm/sdk.md).

## Install

Depend on a release tag:

```toml
[dependencies]
moq-wasm = { git = "https://github.com/nttcom/moq-wasm", tag = "rust-sdk-v0.1.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The SDK runs on tokio. It builds `aws-lc-sys`, so a build needs a C
compiler, and a cross build a C toolchain for the target. It is tested on
Linux x86_64, Linux aarch64 and macOS arm64 with Rust 1.100.

## Send and receive a message

Run `make relay` from the repository root, then run this with the relay's
certificate, `services/relay/keys/cert.pem`. It prints `hello`.

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

`moqt://` connects over raw QUIC, `https://` over WebTransport.

## Publish a broadcast

`media::create_broadcast` registers tracks before it publishes the
namespace, so a SUBSCRIBE the relay forwards at once finds them. A media
source takes frames you encode (H.264, H.265, VP8, VP9, AV1, AAC, Opus) and
publishes them as LOC tracks with the MSF catalog that the browser and
Python SDKs read; the SDK neither encodes nor decodes.

```rust,no_run
use moq_wasm::{Retention, media::{self, MediaSourceOptions}};
use serde_json::json;
use std::time::Duration;

# async fn run(session: &moq_wasm::Session, access_unit: Vec<u8>, timestamp_micros: u64) -> anyhow::Result<()> {
let builder = media::create_broadcast(session, &["anon", "robot-1"]);
let events = builder
    .event_timeline("events", Retention::For(Duration::from_secs(3600)))
    .await?;
let broadcast = builder.announce().await?;

let camera = broadcast.add_media_source(MediaSourceOptions::new("camera"));
camera.configure_video("avc1.42e01f")?;
camera.push_video(access_unit, timestamp_micros)?; // Annex B; a keyframe opens a group
events.append(&json!({"T": 0, "data": "started"})).await?;

broadcast.close().await?;
# Ok(())
# }
```

## Follow broadcasts

`media::watch_broadcasts` runs a handler for each broadcast published under
a prefix, skipping this session's own, and aborts it when the broadcast is
withdrawn. A `RemoteBroadcast` reads the MSF catalog, follows a track
(subscribing again after it ends) and reads an event timeline from the
records the relay keeps.

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
    let mut status = remote.follow("status", FollowOptions::default());
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

[`examples/rust/moq-broadcast`](../../examples/rust/moq-broadcast/README.md)
runs both sides against a relay.

## Tokens and the end of a session

A relay that verifies tokens takes a JWT in `ConnectOptions::token`. Give
`refresh_token` too, and the session asks it for a later token five minutes
before the current one expires, retrying every 30 s:

```rust,no_run
use moq_wasm::{ConnectOptions, RefreshToken, SessionEnd};

async fn later_token(current: String) -> anyhow::Result<String> {
    // Ask your app server for a new token of the same app.
    Ok(current)
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

The session follows GOAWAY and replaces a lost connection for up to 60 s by
itself, sending its namespaces and subscriptions again. `closed()` resolves
when it cannot, when the token expired, or after `close()`.

## Logs

The SDK logs through [`tracing`](https://docs.rs/tracing) and installs no
subscriber: install your own, such as `tracing-subscriber`.
