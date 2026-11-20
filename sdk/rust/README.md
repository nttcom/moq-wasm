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
