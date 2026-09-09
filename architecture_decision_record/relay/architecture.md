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
   `REDIS_URL` (optional), and the authentication mode (`AuthConfig`):
   `AUTH_VTS_URL` + `AUTH_RELAY_TOKEN` enable token verification;
   otherwise `AUTH_DISABLED=true` must be set explicitly or startup fails.
   `AUTH_VTS_URL` wins when both are present.
4. `RelayServer::new_with_config(...)` then:
   - `spawn_client_transport::<moqt::DUAL>(port)` — client-facing endpoint
     accepting both WebTransport and raw QUIC on one port.
   - `spawn_inner_transport::<moqt::QUIC>(inner_port)` — inter-relay endpoint.

`RelayServer` (in `relay_server/`) wires three long-lived pieces:

- `SessionRepository` (shared `Arc<Mutex<_>>`).
- `RelayStore` — `TrackCacheStore` + `SubgroupOpenedNotifierMap`, the shared
  data-plane state.
- `RelayRuntime` — constructs `InterRelayConnectionManager`,
  `UpstreamPublisherResolver`, `IngressCoordinator`, `EgressCoordinator`,
  `EventHandler`, and the cache-eviction job, and returns the relay-wide
  `SessionEvent` sender.

## Control plane

### Session intake
`SessionHandler` runs one accept loop per endpoint and hands every accepted
transport connection to a per-connection task owned by `SessionIntake`:

1. await the `moqt::Accepting` future → `Handshake` (CLIENT_SETUP received,
   SERVER_SETUP not yet sent);
2. `SessionAuthenticator::authenticate(client_setup, accepted_peer)` — in
   `Disabled` mode this yields `VerifiedToken::full_access()`; in `Enabled`
   mode it extracts the JWT, calls the `TokenVerifier`, and checks that
   `is_relay` matches the endpoint (client port ⇔ `false`, inner port ⇔
   `true`). Failures call `Handshake::reject` with `UNAUTHORIZED` (missing or
   rejected token, endpoint mismatch) or `INTERNAL_ERROR` (VTS unreachable);
3. `Handshake::accept()` sends SERVER_SETUP;
4. the session is boxed as `dyn core::session::Session` and added to
   `SessionRepository` as a `NewSession` carrying its `SessionPeer` (`Client`
   or `Relay { relay_id }` — the endpoint it arrived on) and its
   `VerifiedToken`, which later requests are authorized against;
5. for a client token (`is_relay == false`) with an `exp`, the repository
   starts a `SessionExpiryTask` that closes the session with
   `EXPIRED_AUTH_TOKEN (0x18)` when the token expires. Clients are expected to
   obtain a fresh token and reconnect. Relay sessions and the disabled-auth
   `full_access()` token never expire.

### `modules/core` — transport-erased `moqt` facade
The relay never handles `moqt::Session<T>` generically beyond intake. `core`
defines object-safe traits (`Session`, `Publisher`, `Subscriber`, one
`handler::*` trait per control message, `subscription`, `data_receiver`,
`data_sender`) implemented for every `T: TransportProtocol`. Everything past
the repository works with `Box<dyn …>`.

### Event pipeline

```
moqt session ──receive_event()──► Session Event Forwarder (one task per session)
      │  MoqtSessionEvent → SessionEvent { session_id, kind: EventKind::FromSession }
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
- Before dispatching, each session worker runs
  `auth::request_gate::authorize_request` against the session's
  `VerifiedToken` (looked up once when the worker starts). PUBLISH and
  PUBLISH_NAMESPACE need the `publish` claim; SUBSCRIBE, SUBSCRIBE_NAMESPACE
  and standalone FETCH need `subscribe`; joining FETCH and every other
  message pass through (they reference an already authorized request). A
  denied request is answered with the message's `*_ERROR` carrying
  `UNAUTHORIZED (0x1)` and the sequence is never invoked.
- `EventHandler` implements a **reader/worker** structure: the single reader
  only dispatches to per-session unbounded channels, so a slow or blocked
  session can never head-of-line-block another (unit tests in
  `event_handler.rs` pin this). Workers process one event at a time, fully
  awaiting each sequence (including upstream round-trips) — events within a
  session are strictly ordered.
- Events for control messages without relay-side logic yet (GOAWAY,
  MAX_REQUEST_ID, REQUESTS_BLOCKED, PUBLISH_NAMESPACE_CANCEL, PUBLISH_DONE,
  SUBSCRIBE_UPDATE, FETCH_CANCEL, TRACK_STATUS) are logged in the event span
  and dropped by the worker; they have no `sequences` entry.
- Two relay-internal events exist, both reported by the ingest path (not by
  a peer) and routed to the upstream publisher session's worker:
  - `MalformedTrackDetected(session_id, track_key)`, raised by the insert that
    latched the track. Handled by
    `sequences::malformed_track::MalformedTrackCleanup`: remove the
    `ActiveUpstreamSubscription`, send upstream UNSUBSCRIBE (§2.5 MUST), and
    stop ingress via `IngressCommand::StopTrack`. Duplicate reports are
    idempotent (the table entry is only found once).
  - `ProtocolViolationDetected { reason }`, raised when a subgroup object
    carries an Object Status draft-14 §10.2.1.1 does not define. The worker
    closes the session with PROTOCOL_VIOLATION (`Session::close_with_error`);
    the resulting `ProtocolViolation` session event then drives the ordinary
    terminal cleanup.
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

## Authentication and authorization (`modules/auth`)

Building blocks; the session intake wiring is described above, the
per-request authorization gate under "Event pipeline".

- `verified_token.rs` — `VerifiedToken`, the claims returned by the Verify
  Token Service (VTS): `app_id`, optional `publish` / `subscribe` namespace
  paths (already split on `/`; the empty claim `""` is the app root, i.e. an
  empty path), `is_relay`, and `expires_at`.
- `authorize.rs` — `authorize(token, Operation, namespace_tuple)`: rejects a
  namespace whose element contains `/`, requires the first element to equal
  the token's `app_id` unless `is_relay`, then requires the granted path to be
  an element-wise prefix of the remaining tuple.
- `client_setup_token.rs` — `extract_token(&ClientSetup)`: the first
  AUTHORIZATION TOKEN parameter must be `USE_VALUE` with Token Type `0` and a
  UTF-8 value (the JWT). Other alias types are not supported by design.
- `token_verifier.rs` — `TokenVerifier` trait with
  `VerifyError::{Unauthorized, Unavailable}`; the split lets callers map a
  rejected token and an unreachable VTS to different termination codes.
- `vts_token_verifier.rs` — `reqwest` implementation: `POST {AUTH_VTS_URL}`
  with `{"token"}`; 200 → `VerifiedToken`, 401 → `Unauthorized`, anything
  else or a transport error → `Unavailable`. 3 s timeout.
- `session_authenticator.rs` — `SessionAuthenticator::{Disabled, Enabled}`
  built from `AuthConfig`; combines the pieces above into the CLIENT_SETUP
  decision described under "Session intake".
- `request_gate.rs` — `authorize_request` / `reject_unauthorized`, the
  per-request gate described under "Event pipeline".
- `session_expiry_task.rs` — `SessionExpiryTask` (owns its `JoinHandle`,
  aborted on drop) that sleeps until `expires_at` and closes the session via a
  `Weak<dyn Session>` so a departed session is a no-op.

## Data plane

### Shared state (`RelayStore`)
- `TrackCacheStore` — `DashMap<TrackKey, Arc<TrackCache>>`.
- `SubgroupOpenedNotifierMap` — `DashMap<TrackKey, broadcast::Sender<SubgroupOpened>>`
  (capacity 256); ingress announces `SubgroupOpened(SubgroupKey)` when a live
  subgroup stream (or datagram group) starts, egress schedulers listen.

### Ingress (`modules/relay/ingress`)
`IngressCoordinator` consumes `IngressCommand::{Start, StopTrack}`:

- On `Start`, it obtains the upstream session's `Subscriber`, creates the data
  receiver (cancellable via a per-track `watch` stop channel), and hands it to
  `StreamIngressTask` (subgroup streams) or `DatagramReader` (datagrams).
- `StreamIngressTask` runs a per-track factory loop accepting subgroup
  streams. **First-publisher-wins**: a second publisher on an active track is
  ignored (draft-14 §8.2 multiple-publisher dedup is a known TODO), and only
  the owning publisher's `Stop` tears the reader down.
- Readers convert every wire object into a canonical `CachedObject` and insert
  it into `TrackCache`. A SUBGROUP_HEADER is not cached: the reader keeps its
  group id, subgroup id and priority as the per-stream context, opens the
  subgroup in the cache (`open_subgroup`, returning an `OpenSubgroupGuard`) and
  broadcasts `SubgroupOpened`. Only a FIN or an End of Group / End of Track
  object `finish`es the guard; every other end (RESET_STREAM, stop, decode
  error, task abort) drops it, which marks the subgroup aborted: its group is
  never declared complete (draft-14 §10.4.2) and egress resets, rather than
  FINs, the downstream stream (§10.4.3). Header
  types without an explicit subgroup id map to 0, or to the first object's id
  (Type 0x12/0x13/0x1A/0x1B, opened once that object arrives). For End-of-Group
  header types (0x18–0x1D) a clean FIN inserts an EndOfGroup status object at
  `last_id + 1`, so the signal survives header regeneration as data.
- `FetchIngest` inserts each FETCH object one-to-one (`CachedObject::from_fetch_object`
  via `TrackCache::insert`, which registers no knowledge); no header synthesis and
  no per-subgroup delta state.
- The cache's sticky §2.5 malformed latch is only ever set inside an insert,
  so the reader (or fetch fill) that performed the latching insert is always
  present to report `MalformedTrackDetected` into the event pipeline — no
  standing watcher is needed. Downstream, `EgressRunner` watches the same
  latch and terminates subscriptions with PUBLISH_DONE(MALFORMED_TRACK);
  `FetchIngest` bails on the latch and sends upstream FETCH_CANCEL for its
  own fetch.

### Cache (`modules/relay/cache`)
- `CachedObject` (`cached_object.rs`) is the draft-14 §10.2.1 canonical object:
  location, forwarding preference (subgroup id or datagram), publisher
  priority, status, extension headers, payload, and its own `received_at`.
  Wire forms are derived from it at egress (`to_subgroup_object_field` with the
  delta computed from the previously sent id, `to_object_datagram` normalised
  to explicit-id types, `to_fetch_object_field` with subgroup id = object id
  for datagram objects per §10.4.4). `conflicts_with` implements §8.1:
  differing forwarding preference, subgroup, priority or payload, or a status
  move between Normal/EndOfGroup/EndOfTrack, is a Malformed conflict;
  extension changes and Does Not Exist transitions are tolerated duplicates.
- `TrackCache` (`track_cache.rs` + `track_cache/{ledger,open_subgroup}.rs`) is one
  track-level ledger behind a `std::sync::RwLock` that is never held across an
  await: `objects: BTreeMap<Location, Arc<CachedObject>>` (stream and datagram
  objects together, so identity is the key, never an entry), `open_subgroups:
  HashMap<SubgroupKey, usize>` (live-ingest reference counts — the only
  non-data state), and `KnownRanges` (§9.2.1.3 / §9.16 unknown-status
  semantics). One `Notify` per track wakes every waiter on insert, open and
  close; waiters re-check the ledger under a single read guard, so there is no
  check-order race between "object present" and "subgroup closed".
- Knowledge: a live subgroup insert registers only the received position
  (§10.4.2: ids skipped by a non-zero delta cannot be inferred). Each open
  group keeps the largest object id live ingest has seen (`LiveGroup`), and
  closing the last open stream subgroup registers only the tail after it, so
  an evicted position is never re-claimed as known (§9.2.1.3 "their state
  becomes unknown") and a skipped id stays undecided. Fetch fills register
  their requested range only at `Fetch::End` (guarded by the eviction
  generation counter); datagram objects register nothing.
- `next_subgroup_object_or_wait(key, from)` (live egress) returns the next object of that
  subgroup, `Finished` once it closed cleanly, or `Aborted` once it closed without
  a FIN; a subgroup that was never opened (fetch-fill only) therefore never blocks. `fetch_objects` walks
  `[start, end)` in location order, reading positions inside knowledge without
  waiting and waiting past the frontier only while some subgroup of the group
  is open.
- Eviction job (`eviction_job.rs`): every `RELAY_CACHE_EVICT_INTERVAL_SECS`
  (5 s) drop objects older than `RELAY_CACHE_TTL_SECS` (30 s) and release
  knowledge exactly for the removed locations; a `TrackCache` entry is removed
  from the store only when it is empty and `Arc::strong_count == 1`, i.e. no
  ingress/egress holds it — avoiding races with new joiners.

### Egress (`modules/relay/egress`)
`EgressCoordinator` consumes `StartReader` / `StopReader` / `StartFetch` and
keeps one runner per `(subscriber_session_id, downstream_subscribe_id)`
(restart replaces the old runner). `EgressRunner` splits into:

- `EgressScheduler` — listens on the track's broadcast channel and the cache,
  computes the delivery start per draft-14 filter type (`NextGroupStart`,
  `LargestObject`, `AbsoluteStart`, `AbsoluteRange`; an absolute start at or
  below Largest is clamped to Largest+1), and emits one `GroupSendTask` per
  `SubgroupKey`. It subscribes to open events first and then schedules every
  cached group at or after the start, so a group that ingress opened and
  closed before the scheduler existed is still delivered. The start is a
  lower bound in both paths: group ids may begin anywhere and skip values
  (§2.3.1), so the first delivered group is the first one at or above the
  start, not the start group itself.
- `GroupSender` — one task per subgroup: waits for the first object to send,
  only then opens the downstream uni stream (a subgroup that closes empty
  opens nothing), regenerates the SUBGROUP_HEADER from that object's canonical
  properties (explicit subgroup id, priority; extensions always declared
  present so no object can lose its extension headers), and streams objects
  until `next_subgroup_object_or_wait` reports the subgroup finished (FIN
  downstream) or aborted (RESET_STREAM downstream, INTERNAL_ERROR). Datagram groups are
  re-emitted with the downstream track alias.

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
  over raw QUIC (`moqt::QUIC`, certificate verification disabled), presenting
  this relay's own JWT (`AUTH_RELAY_TOKEN`) in CLIENT_SETUP, and registers the
  session as `SessionPeer::Relay` with `VerifiedToken::full_access()`, reusing
  it afterwards. From then on the remote relay behaves like any upstream
  publisher session.

## Key invariants

- **Reader never awaits**: the `EventHandler` reader only routes; all awaiting
  happens in per-session workers. Cross-session deadlock is structurally
  impossible; per-session ordering is FIFO.
- **One upstream subscription per track**: enforced by the
  `UpstreamCreationSerializer` per-track lock with a double-check.
- **SUBSCRIBE_OK matches egress**: the largest location advertised downstream
  is the same value the egress scheduler starts from.
- **Start Location is a lower bound**: egress delivers the first group at or
  above the start whether it arrives as an open event or is already cached;
  neither path requires the start group id itself to exist.
- **First-publisher-wins ingress**: one active reader per track; stop is
  owner-checked.
- **Cache identity is the key**: a cached object is self-contained (§8.1 "MUST
  store all properties"); nothing in the cache refers to an entry by handle,
  so eviction can never orphan a header or resurrect a partial entry.
- **Live state is bounded**: `open_subgroups` is the only non-data cache state,
  it is held by RAII guards in the readers, and live ingest always closes what
  it opened — so every `*_or_wait` is bounded.
- **Ledger lock is never held across an await**.
- **Knowledge follows objects**: evicting an object releases the knowledge at
  exactly that location.
- **Cache lifetime**: a track cache lives while referenced or until TTL
  eviction empties it with `strong_count == 1`.
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
