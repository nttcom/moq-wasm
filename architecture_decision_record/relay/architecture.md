# `relay` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
The `relay` crate is a MoQT relay server (draft-ietf-moq-transport-14, relay
sections) built on the `moqt` crate. It fans publisher tracks out to
subscribers, caches objects per track, serves FETCH from that cache, and
optionally cascades across relays via a Redis-backed route registry.

## Startup path

`main.rs`:
1. `init_logging` (tracing + OpenTelemetry OTLP export).
2. Generate self-signed certs under `relay/keys/` if missing.
3. `RelayConfig::from_env()` — `RELAY_ID`, `RELAY_ADVERTISE_HOST`,
   `RELAY_PORT` (default 4433), `RELAY_INNER_PORT` (default port+1),
   `REDIS_URL` (optional).
4. `RelayServer::new_with_config(...)` then:
   - `spawn_client_transport::<moqt::DUAL>(port)` — client-facing endpoint
     accepting both WebTransport and raw QUIC on one port.
   - `spawn_inner_transport::<moqt::QUIC>(inner_port)` — inter-relay endpoint.

`RelayServer` (in `relay_server/`) wires three long-lived pieces:

- `SessionRepository` (shared `Arc<Mutex<_>>`).
- `RelayStore` — `TrackCacheStore` + `ObjectNotifyProducerMap`, the shared
  data-plane state.
- `RelayRuntime` — constructs `InterRelayConnectionManager`,
  `UpstreamPublisherResolver`, `IngressCoordinator`, `EgressCoordinator`,
  `EventHandler`, and the cache-eviction job, and returns the relay-wide
  `SessionEvent` sender.

## Control plane

### Session intake
`SessionHandler` runs one accept loop per endpoint. Each accepted `moqt`
session is boxed as `dyn core::session::Session` and added to
`SessionRepository` tagged with a `SessionPeer` (`Client` or
`Relay { relay_id }`) — the peer kind of the endpoint it arrived on.

### `modules/core` — transport-erased `moqt` facade
The relay never handles `moqt::Session<T>` generically beyond intake. `core`
defines object-safe traits (`Session`, `Publisher`, `Subscriber`, one
`handler::*` trait per control message, `subscription`, `data_receiver`,
`data_sender`) implemented for every `T: TransportProtocol`. Everything past
the repository works with `Box<dyn …>`.

### Event pipeline

```
moqt session ──receive_event()──► Session Event Forwarder (one task per session)
      │  MoqtSessionEvent → RelaySessionEventResolver → SessionEvent(session_id, handler)
      ▼
EventHandler reader (single task, never awaits handlers)
      │  per-session unbounded channel (lazily created)
      ▼
session worker (one task per session, FIFO)
      │
      ▼
sequences::{PublishNamespace, Subscribe, Fetch, …}.handle(...)
```

- `SessionRepository::start_session_event_forwarding` spawns a forwarder task
  per session that pumps `moqt` events into the relay-wide unbounded channel,
  stopping after `Disconnected` / `ProtocolViolation`.
- `EventHandler` implements a **reader/worker** structure: the single reader
  only dispatches to per-session unbounded channels, so a slow or blocked
  session can never head-of-line-block another (unit tests in
  `event_handler.rs` pin this). Workers process one event at a time, fully
  awaiting each sequence (including upstream round-trips) — events within a
  session are strictly ordered.
- Terminal events (`Disconnected` / `ProtocolViolation`) trigger
  `cleanup_session` (idempotent) and end the worker. Cleanup: remove the
  session from the pub/sub directory, stop affected egress readers, forward
  upstream UNSUBSCRIBE / stop ingress when the last downstream subscriber
  left, withdraw namespace routes for client sessions, then drop the session
  from the repository.

### `modules/sequences` — one struct per control message
Each sequence owns the relay-side protocol logic for one message
(`publish`, `subscribe`, `fetch`, `publish_namespace`,
`publish_namespace_done`, `subscribe_namespace`, `unsubscribe`,
`unsubscribe_namespace`). Shared collaborators:

- `ControlMessageForwarder` — sends control messages on *other* sessions via
  the repository (e.g. forwarding SUBSCRIBE upstream, PUBLISH_NAMESPACE to
  interested subscribers).
- `LocalPubSubDirectory` (trait; `InMemoryLocalPubSubDirectory` impl in
  `tables/`) — the relay's in-memory registry of publish/subscribe namespaces
  (with `PeerKind` so client-owned Redis routes are cleaned up when the last
  *client* leaves), active upstream subscriptions, and downstream
  subscriptions. `remove_session` returns everything cleanup needs.
- `UpstreamCreationSerializer` — per-(namespace, track) async lock.

### SUBSCRIBE sequence (the central flow)
1. **Find-or-create upstream subscription.** Fast path: an
   `ActiveUpstreamSubscription` already exists in the directory. Miss: take
   the per-track serializer lock, re-check (a sibling may have created it),
   otherwise resolve a publisher and send upstream SUBSCRIBE, start ingress,
   and register the upstream subscription — so concurrent subscribers to the
   same track produce exactly one upstream subscription.
2. **Publisher resolution** (`UpstreamPublisherResolver`): local directory
   first (lowest publisher session id wins), then the route registry for a
   remote relay, dialled via `InterRelayConnectionManager`.
3. **Largest Object resolution**: max of the upstream SUBSCRIBE_OK location
   and the local cache's largest location (`resolve_subscribe_largest`). The
   cache is consulted even for a fresh upstream: a publisher that rejoined
   under the same track must not make the relay advertise
   `contentExists=false` and replay stale cache from {0,0}.
4. **Downstream registration + egress start**: register the downstream
   subscription, send `EgressCommand::StartReader` and wait for the runner's
   readiness `oneshot`, then send SUBSCRIBE_OK with the allocated track alias
   and resolved largest location — SUBSCRIBE_OK and egress start always agree.

### FETCH sequence
Resolve the track and object range (Standalone from the message; Relative
Joining from the downstream subscription's start location), reply FETCH_OK,
then delegate to `EgressCommand::StartFetch`, which serves the range entirely
from `TrackCache` over a new uni stream.

## Data plane

### Shared state (`RelayStore`)
- `TrackCacheStore` — `DashMap<TrackKey, Arc<TrackCache>>`.
- `ObjectNotifyProducerMap` — `DashMap<TrackKey, broadcast::Sender<TrackEvent>>`
  (capacity 256); ingress announces new objects, egress schedulers listen.

### Ingress (`modules/relay/ingress`)
`IngressCoordinator` consumes `IngressCommand::{Start, StopTrack}`:

- On `Start`, it obtains the upstream session's `Subscriber`, creates the data
  receiver (cancellable via a per-track `watch` stop channel), and hands it to
  `StreamIngressTask` (subgroup streams) or `DatagramReader` (datagrams).
- `StreamIngressTask` runs a per-track factory loop accepting subgroup
  streams. **First-publisher-wins**: a second publisher on an active track is
  ignored (draft-14 §8.2 multiple-publisher dedup is a known TODO), and only
  the owning publisher's `Stop` tears the reader down.
- Readers append every object into `TrackCache` and broadcast a `TrackEvent`.

### Cache (`modules/relay/cache`)
- `TrackCache`: `group_id → subgroup_id → GroupCache` for streams plus a
  parallel `group_id → GroupCache` map for datagrams; answers
  `largest_location()` and `get_fetch_objects(range)`.
- Eviction job (`eviction_job.rs`): every `RELAY_CACHE_EVICT_INTERVAL_SECS`
  (5 s) evict groups older than `RELAY_CACHE_TTL_SECS` (30 s); a `TrackCache`
  entry is removed from the store only when its `Arc::strong_count == 1`,
  i.e. no ingress/egress holds it — avoiding races with new joiners.

### Egress (`modules/relay/egress`)
`EgressCoordinator` consumes `StartReader` / `StopReader` / `StartFetch` and
keeps one runner per `(subscriber_session_id, downstream_subscribe_id)`
(restart replaces the old runner). `EgressRunner` splits into:

- `EgressScheduler` — listens on the track's broadcast channel and the cache,
  computes the delivery start per draft-14 filter type (`NextGroupStart`,
  `LargestObject`, `AbsoluteStart`, `AbsoluteRange`; an absolute start at or
  below Largest is clamped to Largest+1), and emits `GroupSendTask`s.
- `GroupSender` — opens downstream subgroup streams / datagrams via the
  session's `Publisher` and transmits cached objects in order.

## Cascading relays (`route_registry`, `inter_relay`)

- `RelayRouteRegistry` trait: `NoopRelayRouteRegistry` (single-relay, no
  `REDIS_URL`) or `RedisRelayRouteRegistry` (relay info hash with 15 s TTL
  refreshed by a 5 s heartbeat; namespace-publisher and namespace-subscriber
  routes with the same TTL scheme).
- Only **client-origin** namespaces register routes: `PublishNamespace`
  registers the publisher route and notifies remote subscriber relays;
  `SubscribeNamespace` registers the subscriber route when the first client
  subscriber for a prefix appears.
- `InterRelayConnectionManager` lazily dials the remote relay's inner endpoint
  over raw QUIC (`moqt::QUIC`, certificate verification disabled) and
  registers the session as `SessionPeer::Relay`, reusing it afterwards. From
  then on the remote relay behaves like any upstream publisher session.

## Key invariants

- **Reader never awaits**: the `EventHandler` reader only routes; all awaiting
  happens in per-session workers. Cross-session deadlock is structurally
  impossible; per-session ordering is FIFO.
- **One upstream subscription per track**: enforced by the
  `UpstreamCreationSerializer` per-track lock with a double-check.
- **SUBSCRIBE_OK matches egress**: the largest location advertised downstream
  is the same value the egress scheduler starts from.
- **First-publisher-wins ingress**: one active reader per track; stop is
  owner-checked.
- **Cache lifetime**: a track cache lives while referenced or until TTL
  eviction with `strong_count == 1`.
- **Client-owned routes**: Redis namespace routes are registered/withdrawn
  only for client-origin sessions; relay-learned namespaces are purged locally
  when the last client subscriber for the prefix leaves.

## Testing conventions
Unit tests are colocated (`#[cfg(test)]`) and pin structural invariants —
e.g. reader/worker non-blocking and terminal-event handling in
`event_handler.rs`, largest-location resolution in `sequences/subscribe.rs`,
eviction refcount rules in `cache/store.rs`. Multi-process behaviour
(cascading relays, cache eviction, fetch, multiple publishers, dedup) lives in
the workspace-level `tests/*-e2e` suites driven by `scripts/run-*.mjs`.
