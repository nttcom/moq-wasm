# moqt

Publish and subscribe to tracks over QUIC or WebTransport. Clients offer
MoQT draft-18 first, then draft-14. See the [protocol design](../../docs/architecture/moqt/architecture.md).

## Install

Pin the git dependency to a commit. For the native example below, add these
dependencies to a Rust binary project:

```toml
[dependencies]
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

On `wasm32`, build with `RUSTFLAGS="--cfg web_sys_unstable_apis"`.
For JavaScript, see the [browser SDK](../wasm/js/README.md).

## Send and receive a message

Run `make relay` from the repository root. Put this in your project's
`src/main.rs`. Both sessions trust the local relay's certificate and use the
public namespace `anon/room`. Subscribe before writing to receive the live message.

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

Run `cargo run -- /path/to/moq-wasm/services/relay/keys/cert.pem` from your
project. It prints `hello`. Use `https://` for WebTransport.

## Modules

| Path | Contents |
| --- | --- |
| `moqt::client` | Client sessions, track writers and subscriptions |
| `moqt::` | `Endpoint`, `Session`, `Publisher`, `Subscriber`, `TrackWriter` / `TrackReader`, transport markers |
| `moqt::subscribe` | SUBSCRIBE, SUBSCRIBE_UPDATE, UNSUBSCRIBE and TRACK_STATUS |
| `moqt::publish` | PUBLISH and PUBLISH_DONE |
| `moqt::fetch` | FETCH and FETCH_CANCEL |
| `moqt::namespace` | PUBLISH_NAMESPACE and SUBSCRIBE_NAMESPACE |
| `moqt::object` | Object model and subgroup stream / datagram senders and receivers |
| `moqt::server` | Accepting sessions |
| `moqt::wire` | Draft-14 numeric error codes for bindings that expose them |
