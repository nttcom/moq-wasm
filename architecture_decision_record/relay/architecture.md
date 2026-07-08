# Architecture: `relay` crate

## Status
Accepted (documents the design as of `master`, 2026-07)

## Purpose
`relay` is the MoQT relay server. It terminates MoQT sessions from
publishers and subscribers (end clients and other relays), maintains a
local pub/sub directory, caches objects per track, and fans data out to
downstream subscribers. It extends the `moqt` crate with the
relay-specific behavior described in the relay sections of
draft-ietf-moq-transport-14.

A single relay process listens on two endpoints (see `main.rs`):

- **Client-facing** `DUAL` endpoint (`RELAY_PORT`, default 4433) —
  accepts both WebTransport (browsers) and raw QUIC clients on one port.
- **Inner** `QUIC` endpoint (`RELAY_INNER_PORT`, default port+1) —
  accepts connections from *other relays* for cascading.

## Layer Map

```
relay/src/
├── main.rs / config.rs / logging.rs    Process entry, env config, tracing setup
├── relay_server/                       Composition root
│   ├── server.rs    RelayServer: builds runtime, spawns the two endpoints
│   ├── runtime.rs   RelayRuntime: wires ingress/egress/event handler/eviction
│   └── store.rs     RelayStore: shared state (TrackCacheStore, ObjectNotifyProducerMap)
└── modules/
    ├── core/            Transport erasure over `moqt` (dyn Session/Publisher/
    │                    Subscriber, dyn *Handler, data senders/receivers)
    ├── event_resolver/  moqt::SessionEvent → relay SessionEvent (adds SessionId)
    ├── session_handler.rs      Accept loop per endpoint
    ├── session_repository.rs   SessionId → session registry (+ spans, peer kind)
    ├── event_handler.rs        Reader + per-session worker event dispatch
    ├── sequences/       Per-control-message orchestration (subscribe, publish,
    │                    namespaces, fetch, …) + LocalPubSubDirectory tables
    ├── relay/
    │   ├── ingress/     Pull objects from upstream into the cache
    │   ├── egress/      Push cached/live objects to downstream subscribers
    │   ├── cache/       TrackCacheStore → TrackCache → GroupCache (+ TTL eviction)
    │   └── notifications/  Per-track broadcast of "object arrived" events
    ├── route_registry/  Namespace routing for multi-relay (Noop | Redis)
    ├── inter_relay.rs   Outbound QUIC connections to other relays
    ├── upstream_publisher_resolver.rs  Local publisher vs. remote relay lookup
    └── control_message_forwarder.rs    Send control messages to other sessions
```

## Key Design Decisions

### 1. Transport erasure at the relay boundary (`core/`)
`moqt` exposes sessions as generics (`Session<QUIC>`,
`Session<DUAL>`, …), but the relay must hold sessions of *mixed*
transports in one `SessionRepository`. `core/` defines object-safe
traits (`Session`, `Publisher`, `Subscriber`, per-message `*Handler`
traits) with blanket impls for every `moqt::TransportProtocol`, so all
relay logic downstream of the accept loop works on `Box<dyn …>` and is
transport-agnostic. This is the single place where the generics are
erased.

### 2. Client vs. relay peers are first-class (`SessionPeer`)
Every accepted session is tagged `SessionPeer::Client` or
`SessionPeer::Relay { relay_id }` depending on which endpoint accepted
it. The distinction matters for cleanup and routing: only *client*
publishers/subscribers own route-registry entries, so the directory
tracks peer kind to detect when the last client leaves a namespace
(then the Redis route may be dropped), and relay-learned namespaces are
purged rather than trusted after their subscribers disappear.

### 3. Event pipeline: one reader, one worker per session
All session events funnel into a single unbounded channel consumed by
`EventHandler` (`event_handler.rs`), which implements a
**reader/worker** structure:

- The **reader** never awaits peer responses; it only routes each event
  to a per-session unbounded channel (lazily creating a worker task per
  `SessionId`, tracked in a `JoinSet`). Cross-session deadlock is
  therefore structurally impossible.
- Each **session worker** processes its events strictly in FIFO order,
  fully awaiting each sequence handler (including upstream round
  trips) before the next event. Ordering per session, concurrency
  across sessions; a slow session cannot head-of-line block others.
- Terminal events (`Disconnected`, `ProtocolViolation`) run idempotent
  `cleanup_session` and end the worker; the reader reaps it via
  `JoinSet` and removes the channel.

### 4. Control-plane orchestration lives in `sequences/`
Each MoQT request type has a sequence struct (`Subscribe`, `Publish`,
`PublishNamespace`, `Fetch`, …) whose `handle()` implements the relay
behavior for that message: update the `LocalPubSubDirectory`, respond
via the handler, forward control messages to other sessions
(`ControlMessageForwarder`), start/stop ingress and egress, and cascade
to other relays (`CascadingRelayContext`). `event_handler.rs` is pure
dispatch; the protocol logic is in `sequences/*`.

Supporting pieces:
- `sequences/tables/` — `LocalPubSubDirectory` trait + in-memory
  implementation: who publishes/subscribes what on this relay, upstream
  subscription refcounts (`downstream_subscriber_count`), and what must
  be torn down when a session is removed
  (`RemovedSessionSubscriptions`).
- `sequences/upstream_serializer.rs` — serializes concurrent upstream
  subscription creation so two SUBSCRIBEs for the same track create one
  upstream, not two.

### 5. Data plane is decoupled from the control plane via cache + broadcast
Object flow is **write-once, read-per-subscriber**:

1. **Ingress** (`relay/ingress/`): one ingress per (track, publisher
   session) reads upstream subgroup streams / datagrams, appends every
   object into the `TrackCache`, and publishes a `TrackEvent` on the
   track's broadcast channel (`ObjectNotifyProducerMap`).
2. **Egress** (`relay/egress/`): one runner per (subscriber session,
   subscribe ID) reads objects out of the cache — catching up from its
   start location, then following live notifications — and writes them
   to its downstream session. Fetch is served the same way, straight
   from the cache range.

Consequences: a new subscriber never perturbs the publisher path or
other subscribers; slow subscribers fall behind independently; FETCH
and late joins are cache reads, not upstream round trips.

### 6. Cache structure and eviction
`TrackCacheStore` (DashMap keyed by `TrackKey` = namespace + name) →
`TrackCache` (BTreeMaps of groups, separately for stream subgroups and
datagrams) → `GroupCache` (objects). A periodic eviction job
(`cache/eviction_job.rs`) TTL-evicts old groups and removes a track
entry only when nobody else holds its `Arc` (`strong_count == 1`),
avoiding races with concurrently joining ingress/egress. The cache also
answers `largest_location()`, used to fill SUBSCRIBE_OK — taking the
max of cache and upstream SUBSCRIBE_OK so a publisher rejoin does not
resurrect stale locations (`sequences/subscribe.rs`).

### 7. Multi-relay: route registry + inter-relay connections
Horizontal scaling is namespace-based:

- `route_registry/` — `RelayRouteRegistry` trait mapping
  `track_namespace` → publishing relay (`RelayInfo`) and namespace
  prefixes → subscribing relays. Two implementations: `Noop`
  (standalone relay, everything local) and `Redis` (shared registry;
  enabled when `REDIS_URL` is set). Registration is conflict-checked:
  one active publisher route per namespace.
- `inter_relay.rs` — `InterRelayConnectionManager` lazily dials the
  owning relay's *inner* QUIC endpoint and registers the session as a
  `Relay` peer, reusing one session per remote relay.
- `upstream_publisher_resolver.rs` — on SUBSCRIBE, decides whether the
  upstream is a local publisher session or a remote relay reached
  through the registry + connection manager.
- Namespace announcements (PUBLISH_NAMESPACE etc.) cascade to
  interested relays via `CascadingRelayContext`.

### 8. Shared state is minimal and explicit (`RelayStore`)
Only two things are shared across the whole runtime: the
`TrackCacheStore` and the `ObjectNotifyProducerMap`. Everything else is
owned by a coordinator task and driven by commands
(`IngressCommand`/`EgressCommand` over bounded mpsc channels), keeping
locking narrow. `SessionRepository` sits behind a `tokio::sync::Mutex`
and is the main cross-cutting lock; handlers take it briefly and never
across upstream awaits where avoidable.

### 9. Observability is structured around session spans
Every session gets a root tracing span (`relay.session`) carrying
session id, peer kind, relay id and hostname; every event, sequence,
ingress and egress span is parented to (or linked with, for
cross-session flows like ingress) that span. OpenTelemetry export is
wired in `logging.rs`. When adding relay logic, keep new spans inside
this hierarchy.

## Consequences
- Adding relay behavior for a new control message means: a handler
  trait in `core/handler/`, a `SessionEvent` variant + resolver arm, a
  sequence struct in `sequences/`, and a dispatch arm in
  `event_handler.rs`.
- Per-session FIFO makes reasoning simple but means one session's slow
  upstream round trip delays that session's later events by design.
- The cache is the source of truth for late joiners and FETCH; bugs in
  eviction or `largest_location` surface as wrong SUBSCRIBE_OK content
  or stale replays, so changes there need E2E coverage
  (`scripts/run-media-e2e.mjs`, cache-eviction E2E).

## References
- `spec/draft-ietf-moq-transport-14.txt` (relay-related sections)
- `../moqt/architecture.md` — the underlying protocol crate
- `docker-compose.yml` — two-relay + Redis local topology
