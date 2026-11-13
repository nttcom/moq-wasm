# Design: Python bindings for `moqt`

The bindings are the native module `moq_wasm._native` of the Python SDK
`moq-wasm`; the SDK built on them is designed in [sdk.md](sdk.md).

## Status
Accepted

## Date
2026-10-20

## Scope
A new crate `crates/python` that builds the Python module `moqt` with PyO3, a
helper moved into `moqt` so the module and `broadcast` share one way of serving
published tracks, and the switch of the Meeting transcriber and the AI demo
bots under `examples/python/` from moq-rs to the module.

## Context
The Python bots reach the relay through moq-rs (`moq-rs` 0.4.7, via pipecat).
The Meeting transcriber showed where that stops working against this
repository's relay and pages:

- moq-rs has no joining FETCH. A track the relay already subscribes to for
  someone else delivers nothing until the publisher sends a new group, so the
  transcriber gets no catalog from a participant others already watch.
  draft-14 defines AbsoluteStart `{0, 0}` as the objects published after the
  SUBSCRIBE is processed, so subscribing from group 0 does not help.
- moq-rs refuses a group that it joins in the middle, which is how a late
  subscriber meets any event timeline.
- moq-rs reads LOC as its own payload layout, and does not expose an object's
  extension headers, where draft-ietf-moq-loc-04 puts the Timestamp.
- 0.5 replaced the announcement API with routes, so the version the bots pin
  is already behind.

`moqt` does all of this: `Subscriber::subscribe_current_group`, objects with
their extension headers, and `publish::Origin`, which caches what it publishes
and answers SUBSCRIBE and FETCH. An earlier binding (unmerged branch
`feat/python-bindings`, 2026-09-07) wrapped `Session<DUAL>`, which no longer
exists, and exposed no FETCH.

## Decision

### The crate
`crates/python` (package `python`, like `crates/wasm`) builds a `cdylib`,
the module `moq_wasm._native` of the distribution `moq-wasm`, with PyO3 and
maturin. Its method names follow the browser SDK's `MoqSession`. Every async method returns an asyncio
awaitable through `pyo3-async-runtimes` on its tokio runtime; the module
enters that runtime before it creates the quinn endpoint, which binds through
the ambient runtime. The ABI is `abi3-py311`, the Python version the bots
require. It covers what the Meeting transcriber needs and nothing more:

```python
import moq_wasm

session = await moq_wasm.connect("https://relay.example:4433", verify_certificate=True)

# Namespaces under a prefix, as they are published and withdrawn.
async for event in await session.watch_namespaces(["anon", "room"]):
    event.namespace  # ["anon", "room", "alice"]
    event.active     # False after PUBLISH_NAMESPACE_DONE

# subscribe_current_group sends a joining FETCH for the group the relay holds,
# whose objects come before the live ones.
track = await session.subscribe_current_group(["anon", "room", "alice"], "catalog")
async for obj in track:  # ends with the subscription (PUBLISH_DONE, close())
    obj.group_id, obj.object_id
    obj.payload  # bytes
    # Checked by msf's Catalog::parse: ValueError on a version other than
    # draft-01 or a delta update.
    catalog = moq_wasm.parse_catalog(obj.payload)

# Tracks published through moqt's Origin; the session answers their
# SUBSCRIBEs and FETCHes from its cache. A track is registered before the
# namespace is published, since the relay may forward a SUBSCRIBE at once.
transcript = await session.publish_track(["anon", "room", "bots", "transcriber"], "transcript", retention_seconds=3600)
await session.announce(["anon", "room", "bots", "transcriber"])
await transcript.write(payload, new_group=True)
await transcript.write(payload)

await session.close()
```

Left out: the server side (`listen`), raw request handlers, datagram tracks,
extension headers (the transcriber reads no LOC property), PUBLISH from a peer
(rejected as `Uninterested`, as the recorder does), SUBSCRIBE_UPDATE and
standalone FETCH.

### Authorization tokens
A bot that serves a temporary app's room connects with that app's token and
must outlive it (docs/architecture/ai-bots/on-demand-rooms.md "Temporary
apps"):

```python
session = await moq_wasm._native.connect(url, token=jwt)
await session.refresh_token(later_jwt, app_id="temp-3f9a…")
```

The SDK's `moq_wasm.connect(url, token=, refresh_token=)` calls these when the
token is due (sdk.md "Core").

`connect` hands `moqt` an `AuthorizationTokenSource::provider` that reads the
session's current token, so a connection the session opens by itself (to
follow GOAWAY, or after the relay closed it for an expired token) presents the
latest one. `refresh_token` sends the relay's in-session refresh, a
TRACK_STATUS for `[app_id]` that carries the token (docs/architecture/relay/
architecture.md "Token refresh"), and makes the token current once the relay
accepts it. Getting a later token is the caller's job: the module knows
nothing of the token mint.

### One event loop per session
A session has one event receiver, so the module runs one loop per session:

- Each event goes first to the `broadcast::RelayConnection` of every media
  broadcast the session built (sdk.md "Publishing media"), which answers
  what concerns the broadcast's tracks.
- SUBSCRIBE, UNSUBSCRIBE, FETCH, FETCH_CANCEL and TRACK_STATUS go to the
  session's `Origin` through `Origin::handle`. A SUBSCRIBE for a track the
  origin does not have is rejected with TRACK_DOES_NOT_EXIST.
- PUBLISH_NAMESPACE and PUBLISH_NAMESPACE_DONE go through
  `moqt::namespace::NamespaceWatchers` to every `subscribe_namespace`
  iterator whose prefix they match. PUBLISH_NAMESPACE is answered `ok`. An
  iterator is registered before its SUBSCRIBE_NAMESPACE is sent, since the
  relay answers with the namespaces it already knows.
- `Disconnected` and `ProtocolViolation` end every iterator.

`Origin::handle` is shared with `broadcast::RelayConnection`, so the module
holds no copy of the publisher-side routing.

### Build and CI
- `[lib] test = false`: an extension module cannot link as a test binary, so
  the module is tested with pytest.
- The module is abi3, which `pyo3-build-config` configures without an
  interpreter. `.cargo/config.toml` sets `PYO3_NO_PYTHON` and
  `PYO3_BUILD_EXTENSION_MODULE`, so `cargo build` and `clippy` of the workspace
  need no Python and link the crate as an extension module.
- pytest runs the module against a relay and a VTS started from this
  workspace on free ports (`tests/conftest.py`), in the `python.yml` workflow
  when the crate, `moqt`, the relay or the VTS change.

### Using it from the bots
A bot depends on the module by path (`moq-wasm = { path = "../../../crates/python" }`
in `[tool.uv.sources]`), and uv builds it with maturin, so building a bot
needs a Rust toolchain. `examples/python/Dockerfile` gains a builder stage
that compiles the wheel.

The Meeting transcriber is the first user:
- participants come from `watch_namespaces`, which also reports when they
  leave;
- catalogs are subscribed with `subscribe_current_group`;
- the transcript is published through `session.publish_track`, so a late joiner's
  FETCH is answered from the cache.

The chat moderation bot is the second:
- the page's chat is subscribed whenever `watch_namespaces` reports the
  chat namespace, and again when the relay ends the track while the namespace
  stays published;
- the chat is plain UTF-8 text, one message per group, instead of moq-rs's
  compressed JSON stream; a verdict names a message by its object's Location;
- the verdicts and the System One status are published through `session.publish_track`
  instead of the broadcast inside pipecat's `MOQTransport`.

Camera detection and PTZ tracking follow: each subscribes to its page's
prompt and the camera's video while their namespaces are published, and
publishes its event timeline, System One status and, for PTZ, the ONVIF
commands through `session.publish_track`. A video subscription that joins a group in
the middle decodes nothing until the next group, which starts at a keyframe.

## Consequences

### Positive
- Python code meets the relay through the same implementation as the relay,
  the recorder and the browser.
- The transcriber reads the catalog of a participant others already watch,
  and serves its transcript to late joiners.

### Negative
- Building a bot needs Rust and takes minutes instead of seconds.
- The module's API is a second public surface of `moqt` to keep working
  through changes such as draft-18.

## Alternatives Considered

### Re-send the Meeting catalog periodically
`broadcast` re-sends its catalog every 30 s, so the Meeting could too. The
transcriber would then see each catalog within one period. It leaves moq-rs's
other gaps (mid-group joins, extension headers) in place and makes the
transcriber's start depend on the period.

### Subscribe to the default audio track name
Works without any catalog, but breaks for a room whose catalog was edited.

### Add joining FETCH to moq-rs
It is the fix for every moq-rs user, but its schedule is not ours, and it
would not cover the extension headers.

### A thin one-to-one wrapper of `Session`, `Subscriber` and `Publisher`
It would expose every handler to Python and leave each bot to write the event
routing and the joining FETCH that the recorder and `broadcast` already have in
Rust.
