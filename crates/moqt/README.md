# moqt

Low-level Media over QUIC Transport (MoQT) implementation over QUIC, WebTransport, and browser WebTransport/WebSocket. Clients offer draft-18 first, then draft-14.

> **Building an application or bot?** Start with the [`moq-wasm` Rust SDK](https://github.com/nttcom/moq-wasm/blob/master/sdk/rust/README.md) for automatic reconnects, token refresh, and MSF broadcasts (or the [JavaScript](https://github.com/nttcom/moq-wasm/blob/master/sdk/js/js/README.md) / [Python](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/README.md) SDKs). Use `moqt` directly when you need low-level protocol control or are building a server/relay.

## Install

Pin to a commit in `Cargo.toml`:

```toml
[dependencies]
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }

# For native targets (omit `rt-multi-thread` on wasm32):
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

- **Optional feature**: `self-signed-cert` enables self-signed certificate generation (for local/test servers).
- **WebAssembly (`wasm32`)**: build with `RUSTFLAGS="--cfg web_sys_unstable_apis"`.

## Quickstart: Send and receive a message

Start a local relay (`make relay`), then run this example with `services/relay/keys/cert.pem` — it prints `hello` (subscribe before writing because live subscriptions receive objects published afterwards):

```rust,no_run
use moqt::{CertificateVerification, ClientConfig, client::Session};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cert = std::env::args_os().nth(1).ok_or("pass the relay certificate path")?;
    let config = ClientConfig {
        certificate_verification: CertificateVerification::TrustedCertificates(cert.into()),
        ..Default::default()
    };
    let url = "moqt://127.0.0.1:4433";
    let publisher = Session::connect(url, &config).await?;
    let subscriber = Session::connect(url, &config).await?;
    let namespace = ["anon", "room"];

    let writer = publisher.publish_track(&namespace, "chat", Default::default()).await?;
    publisher.announce(&namespace).await?;
    let messages = subscriber.subscribe(&namespace, "chat", Default::default()).await?;
    writer.write_group("hello".into()).await?;
    let message = messages.next().await?.ok_or("track ended before the message")?;
    println!("{}", std::str::from_utf8(&message.payload)?);

    subscriber.close().await;
    publisher.close().await;
    Ok(())
}
```

Run `cargo run -- /path/to/moq-wasm/services/relay/keys/cert.pem`. Use `moqt://` for raw QUIC or `https://` for WebTransport.

## Modules

| Path | Use for |
| --- | --- |
| `moqt::client` | High-level client `Session`, track writers, subscriptions, and event handlers |
| `moqt::*` | Protocol primitives (`Endpoint`, `Session`, `Publisher`, `Subscriber`, `TrackWriter`/`TrackReader`, transport markers) |
| `moqt::subscribe` | `SUBSCRIBE`, `SUBSCRIBE_UPDATE`, `UNSUBSCRIBE`, `TRACK_STATUS` |
| `moqt::publish` | `PUBLISH`, `PUBLISH_DONE`, `Origin` fan-out and retention |
| `moqt::fetch` | `FETCH`, `FETCH_CANCEL` |
| `moqt::namespace` | `PUBLISH_NAMESPACE`, `SUBSCRIBE_NAMESPACE` |
| `moqt::object` | Object model and subgroup stream / datagram senders and receivers |
| `moqt::executor` | Cross-target task spawning, sleep, and monotonic clock (Tokio on native, browser runtime on `wasm32`) |
| `moqt::server` | Accepting inbound server/relay sessions |
| `moqt::wire` | Draft-14 numeric error codes for language bindings |

Design: [docs/architecture/moqt/architecture.md](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/moqt/architecture.md).
