# moqt

Media over QUIC Transport
([draft-ietf-moq-transport-14](https://datatracker.ietf.org/doc/draft-ietf-moq-transport/14/))
sessions with a publisher and subscriber API, over raw QUIC or WebTransport.
Design: [architecture document](../../docs/architecture/moqt/architecture.md).

## Install

Depend on it through git and pin a commit; the crate is not on crates.io:

```toml
[dependencies]
moqt = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
```

On `wasm32`, build with `RUSTFLAGS="--cfg web_sys_unstable_apis"`: web-sys
exposes its WebTransport bindings only behind that flag. The browser bindings
built on this crate are in [`crates/wasm`](../wasm/README.md).

## Publish a track

`moqt://` connects over raw QUIC and `https://` over WebTransport. `publish`
returns once the relay answers PUBLISH_OK; `TrackWriter` then sends each
payload as a group of one object.

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

drop(session);
endpoint.wait_idle().await;
# Ok(())
# }
```

Seed the first group ID from the clock: a relay keeps the groups it has
cached, so a publisher that reconnects and starts again from group 0 repeats
locations the relay already holds.

Await `Endpoint::wait_idle` after dropping the sessions and before the process
exits. The close is sent by a background task; exiting first leaves the peer
to notice through its idle timeout.

## Subscribe to a track

```rust,no_run
use moqt::{
    ClientConfig, Endpoint, QUIC, TrackReader,
    object::DataReceiver,
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
let DataReceiver::Stream(factory) = subscriber.accept_data_receiver(&subscription).await? else {
    anyhow::bail!("the track is delivered over datagrams");
};

let mut reader = TrackReader::new(factory);
while let Some(object) = reader.next_object().await? {
    println!("group {} object {}: {:?}", object.group_id, object.object_id, object.payload);
}
# Ok(())
# }
```

`FilterType::NextGroupStart` starts at the next group, so a video track begins
at a key frame when the publisher starts each group with one.
`FilterType::LargestObject` (the default) starts at the object after the
latest one (draft-14 §5.1.2), so a track that only changes occasionally shows
nothing until its next object.

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
