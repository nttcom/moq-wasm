# moqt

Core **Media over QUIC Transport (MoQT)** library in Rust supporting `draft-ietf-moq-transport-18` and `draft-ietf-moq-transport-14` over QUIC, WebTransport, and QMux over WebSocket.

> **Building an application, AI bot, or robot?** Start with the [`moq-wasm` Rust SDK](https://github.com/nttcom/moq-wasm/blob/master/sdk/rust/README.md) (or the [JavaScript](https://github.com/nttcom/moq-wasm/blob/master/sdk/js/js/README.md) / [Python](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/README.md) SDKs) for automatic reconnects, token refresh, and MSF broadcasts. Use `moqt` directly for relays, servers, or low-level protocol control.

## What it provides

- **Dual-draft negotiation**: Clients offer draft-18 first, then fall back to draft-14 (`DEFAULT_VERSIONS`).
- **Multi-transport**: Native (`moqt::NATIVE`) supports QUIC (`moqt://`), WebTransport (`https://`), and a Happy Eyeballs-style QMux over WebSocket (`wss://`) fallback (200 ms head start native, 500 ms browser); `moqt::QUIC` restricts to raw QUIC. Browser (`wasm32`, `moqt::BROWSER`) supports WebTransport and WebSocket clients.
- **Managed client (`moqt::client`)**: Owns the event loop and routes subscriptions via `Origin` (`Session::connect`, `publish_track`, `subscribe`, `fetch`, `announce`, `watch_namespaces`).
- **Low-level primitives (`moqt::*`, `moqt::server`)**: `Endpoint`, `Handshake`, `Session`, `Publisher`, `Subscriber`, and `Origin` for relays and custom event loops.

## Install

Pin `moqt` to a commit in `Cargo.toml`:

```toml
[dependencies]
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }

# Native targets require Tokio (omit `rt-multi-thread` on wasm32):
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

- **Native**: Requires a C compiler for `aws-lc-sys` (via `rustls`).
- **Optional feature**: `self-signed-cert` (native only) enables `moqt::server::{CertificateFiles, write_self_signed_cert}`.
- **`wasm32-unknown-unknown`**: Build with `RUSTFLAGS="--cfg web_sys_unstable_apis"`. Only `CertificateVerification::NativeRoots` is supported.

## Quickstart: Publish and subscribe to a track

1. Start a local relay from the repository root with `make relay` (runs VTS + relay using `python3` and generates the self-signed certificate at `services/relay/keys/cert.pem`).
2. Run the program below with `cargo run -- /path/to/moq-wasm/services/relay/keys/cert.pem`:

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

    // Subscribe before writing: live subscriptions only receive objects published afterwards.
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

## Modules

### Session & transport

| Module | Use for |
| --- | --- |
| `moqt::client` | Managed client `Session`, `TrackWriter`, `Subscription`, `Fetch`, `TrackFollower`, `NamespaceWatch`, `TrackWatch`, `EventHandler` |
| `moqt::*` | Low-level `Endpoint`, `Session`, `ClientConfig`, `AuthorizationTokenSource`, `Publisher`, `Subscriber`, transport markers (`NATIVE`, `QUIC`, `BROWSER`) |
| `moqt::server` | Inbound `ServerConfig`, `Accepting`, `Handshake`, and (`self-signed-cert`) certificate generation |
| `moqt::executor` | Cross-target `spawn`, `JoinHandle`, `AbortOnDrop`, `sleep`, monotonic `now()`, `system_time()`, and `MaybeSend`/`MaybeSync` |
| `moqt::wire` | Draft-14 numeric error codes for language bindings |

### Protocol messages & data

| Module | Use for |
| --- | --- |
| `moqt::publish` | `PUBLISH`, `PUBLISH_DONE`, and `Origin` (fan-out, group numbering, and object retention) |
| `moqt::subscribe` | `SUBSCRIBE`, `SUBSCRIBE_UPDATE`, `UNSUBSCRIBE`, `TRACK_STATUS` |
| `moqt::fetch` | `FETCH`, `FETCH_CANCEL`, and fetched object streams |
| `moqt::namespace` | `PUBLISH_NAMESPACE`, `SUBSCRIBE_NAMESPACE` |
| `moqt::object` | Object model, subgroup streams, datagrams, extension headers, and `MalformedTrackError` |

## Key caveats

- **Pick one read mode per `Subscription`**: Call either `next()` (objects/datagrams) or `next_group()` (subgroup streams). Mixing both fails, and `next_group()` fails if a datagram arrives.
- **Safe drop semantics**: Dropping a send stream before `close`/`reset` resets it with code `0` instead of sending a partial subgroup `FIN`.
- **Track defaults**: `PublishTrackOptions::default()` sets 60 s retention, priority `128`, `publish: false`, `datagram: false`, and `on_demand: false`. `Session::publish_track` seeds the initial Group ID from wall-clock microseconds so restarts do not collide with relay caches.
- **Auth & `GOAWAY`**: Set `ClientConfig::authorization_token` (`AuthorizationTokenSource`) for namespaces outside `anon/**`. `follow_go_away` defaults to `true`.
- **Limits**: Control requests time out after 10 s (`RequestFailure::Timeout` keeps the session open); payloads and extension headers cap at 16 MiB.

## Further reading

- Architecture: [docs/architecture/moqt/architecture.md](https://github.com/nttcom/moq-wasm/blob/master/docs/architecture/moqt/architecture.md)
- Relay built on `moqt`: [services/relay/README.md](https://github.com/nttcom/moq-wasm/blob/master/services/relay/README.md)
