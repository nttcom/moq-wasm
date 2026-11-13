# moqt

Media over QUIC Transport
([draft-ietf-moq-transport-14](https://datatracker.ietf.org/doc/draft-ietf-moq-transport/14/))
sessions with a publisher and subscriber API, over raw QUIC or WebTransport.
Design: [architecture document](../../docs/architecture/moqt/architecture.md).

## Install

The crate is not on crates.io; depend on it through git and pin a commit:

```toml
[dependencies]
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
```

On `wasm32`, build with `RUSTFLAGS="--cfg web_sys_unstable_apis"`. The browser
bindings built on this crate are in [`crates/wasm`](../wasm/README.md).

## Publish a track

`moqt://` connects over raw QUIC and `https://` over WebTransport.

```rust,no_run
use bytes::Bytes;
use moqt::{ClientConfig, Endpoint, QUIC, TrackWriter, publish::PublishOption};

# async fn run() -> anyhow::Result<()> {
let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig::default())?;
let session = endpoint.connect("moqt://relay.example.com:4433").await?.await?;
let publisher = session.publisher();
let subscription = publisher
    .publish("example/room".to_string(), "chat".to_string(), PublishOption::default())
    .await?;

let first_group_id = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)?
    .as_micros() as u64;
let mut writer = TrackWriter::new(publisher.create_stream(&subscription), first_group_id);
writer.write_group(Bytes::from_static(b"hello")).await?;
writer.finish().await?;

session.close().await;
# Ok(())
# }
```

- Seed the first group ID from the clock: a relay keeps the groups it has
  cached, so a publisher that restarts from group 0 repeats their locations.
- Await `Session::close` before the process exits; otherwise the peer notices
  only through its idle timeout.

## Subscribe to a track

```rust,no_run
use moqt::{
    ClientConfig, Endpoint, QUIC,
    subscribe::{FilterType, SubscribeOption},
};

# async fn run() -> anyhow::Result<()> {
let endpoint = Endpoint::<QUIC>::create_client(&ClientConfig::default())?;
let session = endpoint.connect("moqt://relay.example.com:4433").await?.await?;
let mut subscriber = session.subscriber();
let option = SubscribeOption {
    filter_type: FilterType::NextGroupStart,
    ..SubscribeOption::default()
};
let subscription = subscriber
    .subscribe("example/room".to_string(), "chat".to_string(), option)
    .await?;

let mut reader = subscriber.track_reader(&subscription);
while let Some(object) = reader.next_object().await? {
    println!("group {} object {}: {:?}", object.group_id, object.object_id, object.payload);
}
# Ok(())
# }
```

`FilterType::NextGroupStart` starts at the next group, so a video track begins
at a key frame. `FilterType::LargestObject` (the default) starts at the object
after the latest one (draft-14 §9.7).

## Modules

| Path | Contents |
| --- | --- |
| `moqt::` | `Endpoint`, `Session`, `Publisher`, `Subscriber`, `TrackWriter` / `TrackReader`, transport markers |
| `moqt::subscribe` | SUBSCRIBE, SUBSCRIBE_UPDATE, UNSUBSCRIBE and TRACK_STATUS |
| `moqt::publish` | PUBLISH and PUBLISH_DONE |
| `moqt::fetch` | FETCH and FETCH_CANCEL |
| `moqt::namespace` | PUBLISH_NAMESPACE and SUBSCRIBE_NAMESPACE |
| `moqt::object` | Object model and subgroup stream / datagram senders and receivers |
| `moqt::server` | Accepting sessions |
| `moqt::wire` | Raw message codecs |
