# `relay` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
The `relay` crate is a MoQT relay server (draft-ietf-moq-transport-14, relay
sections) built on the `moqt` crate. It fans publisher tracks out to
subscribers, caches objects per track, serves FETCH from that cache, and
optionally cascades across relays via a Redis-backed route registry.

## Module layout

`src/modules/` has one directory per concern; `relay_server/` (outside
`modules/`) owns startup, the transport accept loop and the runtime wiring.

| Directory | Holds |
| --- | --- |
| `session/` | The session as the relay sees it: `Session` / `Publisher` / `Subscriber` traits implemented by `moqt::Session` and by the test mocks, the per-message `handler::*` traits, data stream adapters, `SessionRepository`, `SessionEvent` |
| `domain/` | Relay vocabulary and state shared by every layer: `TrackKey`, `SessionId`, `SessionPeer`, the pub/sub directory and its entries (error codes are `moqt`'s typed enums) |
| `auth/` | Token verification, claims, per-request authorization, token refresh, session expiry |
| `control_plane/` | Control-message processing: `EventHandler`, one `sequences/*` struct per message, `ControlMessageForwarder`, `UpstreamPublisherResolver`, `UpstreamCreationSerializer` |
| `data_plane/` | Objects in flight: `ingress/`, `cache/`, `egress/`, and the data-plane integration tests |
| `cascading/` | Relay-to-relay: `RelayRouteRegistry` (Redis / no-op) and `InterRelayConnectionManager` |
| `observability/` | `StatsCollector` (snapshot of sessions, tracks, subscriptions and process memory) and `StatsPublishTask`, which publishes it once per second |
| `test_support/` | `cfg(test)` mocks, the data-plane `RelayHarness` and directory fixtures shared across modules |

Dependencies point downwards in this order: `control_plane` → `cascading`,
`data_plane` → `session`, `auth` → `domain`. `observability` only reads
`session`, `data_plane` and `domain` state. `auth` and `session` reference
each other only through `VerifiedToken` / `SessionExpiryTask`.

## Startup path

`main.rs`:
1. `init_logging` (tracing + OpenTelemetry OTLP export).
2. Generate self-signed certs under `RELAY_CERT_DIR` (default `crates/relay/keys/`) if missing.
3. `RelayConfig::from_env()` — `RELAY_ID`, `RELAY_ADVERTISE_HOST`,
   `RELAY_PUBLIC_HOST` (the `host[:port]` clients connect to, default
   `localhost:RELAY_PORT`), `RELAY_PORT` (default 4433), `RELAY_INNER_PORT`
   (default port+1), `REDIS_URL` (optional), and the authentication settings
   (`AuthConfig`):
   `AUTH_VTS_URL` and `AUTH_RELAY_TOKEN` are both required; startup fails
   without them. There is no unauthenticated mode: a relay that should serve
   only anonymous clients still needs a VTS to reject the tokens it cannot
   verify. `AUTH_MAX_TOKEN_TTL_SECONDS` (default 86400) and
   `AUTH_CLOCK_LEEWAY_SECONDS` (default 60) form the `ClaimPolicy` the relay
   applies to token times.
4. `RelayServer::new_with_config(...)` then:
   - `spawn_client_transport::<moqt::DUAL>(port)` — client-facing endpoint
     accepting both WebTransport and raw QUIC on one port.
   - `spawn_inner_transport::<moqt::QUIC>(inner_port)` — inter-relay endpoint.
   - `spawn_stats_publisher(relay_id, inner_port)` — see "Observability".

`RelayServer` (in `relay_server/`) wires two long-lived pieces:

- `SessionRepository` (shared `Arc<Mutex<_>>`) — one `SessionEntry` per
  session: the session, its span, `SessionPeer`, `VerifiedToken`, expiry task
  and event forwarder task.
- `RelayRuntime` — constructs the shared data-plane state (`TrackCacheStore`),
  `InterRelayConnectionManager`, `UpstreamPublisherResolver`, `IngressCoordinator`, `EgressCoordinator`,
  `EventHandler` (with the `WorkerDeps` every session worker clones), and the
  cache-eviction job, and returns the relay-wide `SessionEvent` sender.

## Shutdown

Session migration, draft-14 §3.5 (`relay_server/drain.rs`). On the first
SIGINT (Ctrl+C) or SIGTERM (`docker stop`, Kubernetes) `main.rs` stops the
client transport's accept loop (`SessionHandler::stop_accepting`, after which
the endpoint refuses every connection attempt at the transport level so a
client is turned away at once;
the inner endpoint keeps accepting) and calls
`RelayServer::drain(RELAY_DRAIN_TIMEOUT_SECS)`, which

1. marks the relay `draining` in the route registry, so other relays stop
   routing new cascaded subscriptions to it and no longer pick it as a
   migration target;
2. asks the registry for a migration target: the `public_host` of the other
   `active` relay with the fewest client sessions (lowest id among equals),
   or none standalone / when every other relay is draining;
3. sends GOAWAY to every `SessionPeer::Client` session with the target in the
   scheme the client connected with (`moqt://host` for raw QUIC,
   `https://host` for WebTransport, from `Session::url_scheme`), or with
   an empty URI when there is no target, which tells the client to reconnect
   to the same URI once the relay is back;
4. from the GOAWAY on, releases the PUBLISH-initiated upstream
   subscriptions like the SUBSCRIBE-initiated ones (draft-22 §6.6.1): each
   client session's worker unsubscribes the tracks it published that nobody
   watches (`ReleaseUnwatchedPublishes`), and the last downstream subscriber
   leaving a track unsubscribes the rest. A publisher publishing on both
   relays (draft-22 §7.6.1) takes that UNSUBSCRIBE as the sign that it can
   stop sending here;
5. waits on `SessionRepository::client_session_count` until no client
   session is left, which happens when every client closed its session after
   migrating;
6. closes the client sessions still present at the timeout with
   `GOAWAY_TIMEOUT` (§3.5 SHOULD).

`main.rs` then waits up to 3 s for the client endpoint to go idle
(`SessionHandler::wait_idle`), so the CONNECTION_CLOSE of a session closed at
the timeout leaves the socket before the process exits. A second signal
during the drain exits at once. Relay-peer sessions receive a GOAWAY with an
empty URI and are not waited for: the remote relay answers it by subscribing
the other publishers of the tracks this relay feeds it (see "GOAWAY
sequence"), so those tracks survive the close. A client whose handshake
completed just before the accept loop stopped is closed at the timeout like a
client that ignored its GOAWAY.

## Control plane

### Session intake
`SessionHandler` (`relay_server/session_handler.rs`) runs one accept loop per endpoint and hands every accepted
transport connection to a per-connection task owned by `SessionIntake`:

1. await the `moqt::server::Accepting` future → `Handshake` (CLIENT_SETUP received,
   SERVER_SETUP not yet sent);
2. `SessionAuthenticator::authenticate(authorization_tokens, accepted_peer)`
   extracts the JWT from the CLIENT_SETUP tokens (`Handshake::authorization_tokens()`), calls the `TokenVerifier`, and checks that `is_relay` matches the
   endpoint (client port ⇔ `false`, inner port ⇔ `true`). A client that presents no token on the client endpoint is
   accepted with `VerifiedToken::anonymous()` (scope `anon/**`); a missing
   token on the inter-relay endpoint is rejected. Other failures call
   `Handshake::reject` with `UNAUTHORIZED` (rejected token, endpoint mismatch)
   or `INTERNAL_ERROR` (VTS unreachable);
3. `Handshake::accept()` sends SERVER_SETUP;
4. the session is boxed as `dyn session::Session` and added to
   `SessionRepository` as a `NewSession` carrying its `SessionPeer` (`Client`
   or `Relay` — the endpoint it arrived on) and its
   `VerifiedToken`, which later requests are authorized against;
5. for a client token (`is_relay == false`) with an `exp`, the repository
   starts a `SessionExpiryTask` that closes the session with
   `EXPIRED_AUTH_TOKEN (0x18)` when the token expires. A client that wants to
   outlive its token refreshes it in-session (see "Token refresh" below);
   otherwise it obtains a fresh token and reconnects. Relay sessions never
   expire; the relay's own outbound inter-relay sessions carry
   `VerifiedToken::full_access()`.

### Token refresh
draft-14 has no message for renewing an authorization token, and a publisher
sends no request after PUBLISH that could carry one. The relay therefore
repurposes TRACK_STATUS, the only request that has no side effects, as the
carrier: a client sends TRACK_STATUS (namespace `[app_id]`, track name
`update_auth_token` by convention; the relay reads neither) with the new JWT as
its AUTHORIZATION TOKEN parameter. A TRACK_STATUS without an AUTHORIZATION
TOKEN is a status query instead (see "TRACK_STATUS sequence"). The session worker runs
`auth::token_refresh::refresh_token`: extract the token, verify it with the
`TokenVerifier`, require a client (non-relay) session and a non-relay token
whose `app_id` matches the current one. On success
`SessionRepository::replace_verified_token` swaps the session's
`VerifiedToken` and restarts its `SessionExpiryTask` at the new `exp`, the
worker adopts the new token for later requests, and TRACK_STATUS_OK is sent
with Track Alias 0 and Content Exists false — it reports no track status. On
failure TRACK_STATUS_ERROR carries `NOT_SUPPORTED (0x3)` on an inter-relay
session, `MALFORMED_AUTH_TOKEN (0x10)` for an unsupported
alias type / token type / non-UTF-8 value, `UNAUTHORIZED (0x1)` for a rejected
token, a relay token or an `app_id` mismatch, and `INTERNAL_ERROR (0x0)` when
the VTS is unreachable; the session keeps its current token. The refresh has
no retroactive effect: publishes and subscriptions already established stay
up, only later requests and the expiry follow the new token. AUTHORIZATION
TOKEN parameters on any other message are still ignored.

### `modules/session` — the session as the relay sees it
`session` defines traits (`Session`, `Publisher`, `Subscriber`, a `handler::*`
trait per control message, `subscription`, `data_receiver`, `data_sender`)
implemented by the `moqt` types and by the mocks in `test_support`, so the
relay logic is unit-tested against recorded control messages instead of a live
transport. Everything past the repository works with `Box<dyn …>`;
`MoqtSessionEvent` carries the `moqt` handlers without a relay-side trait
(GOAWAY, MAX_REQUEST_ID, PUBLISH_DONE, …) as they are. The same module holds
`SessionRepository` and the relay-wide `SessionEvent` (`session_event.rs`),
so everything the relay knows about a session lives in one place.

### Event pipeline

```
moqt session ──receive_event()──► Session Event Forwarder (one task per session)
      │  MoqtSessionEvent → SessionEvent { session_id, kind: EventKind::FromSession }
      ▼
EventHandler reader (single task, never awaits handlers)
      │  per-session unbounded channel (created on SessionRegistered)
      ▼
session worker (one task per session, FIFO)
      │
      ▼
sequences::{PublishNamespace, Subscribe, Fetch, …}.handle(...)
```

- `SessionRepository::add` sends `EventKind::SessionRegistered` on the
  relay-wide channel before `SessionEventForwardTask::run` spawns the
  session's forwarder task (aborted when the session's entry leaves the
  repository), which pumps `moqt` events into the same channel,
  stopping after `Disconnected` / `ProtocolViolation`. The reader spawns a
  session's worker only on `SessionRegistered` and drops events for a session
  that has no worker: every other event for a session (from its forwarder or
  from ingress, which only starts for registered sessions) is sent after the
  registration, so it can only lack a worker once the worker has exited on its
  terminal event. A late relay-internal event for a departed session (e.g. a
  reader reporting `MalformedTrackDetected` after the publisher disconnected)
  therefore never resurrects a worker.
- Each session worker opens the event's `relay.session.event` span, the one
  place that lists the event's fields, and logs `Received session event` in
  it. Before dispatching, it runs `auth::request_gate::authorize_request` against the session's
  `VerifiedToken` (looked up once when the worker starts and replaced when a
  TRACK_STATUS token refresh succeeds). PUBLISH and
  PUBLISH_NAMESPACE need the `publish` claim; SUBSCRIBE, SUBSCRIBE_NAMESPACE,
  standalone FETCH and a TRACK_STATUS status query (no AUTHORIZATION TOKEN)
  need `subscribe`; a token-refresh TRACK_STATUS, joining FETCH and every other
  message pass through (they reference an already authorized request). A
  denied request is answered with the message's `*_ERROR` carrying
  `UNAUTHORIZED (0x1)` and the sequence is never invoked.
- `EventHandler` implements a **reader/worker** structure (`event_handler.rs` is the
  reader, `event_handler/session_worker.rs` the per-session worker,
  `event_handler/session_cleanup.rs` the terminal cleanup): the single reader
  only dispatches to per-session unbounded channels, so a slow or blocked
  session can never head-of-line-block another. Workers process one event at a time, fully
  awaiting each sequence (including upstream round-trips) — events within a
  session are strictly ordered. The one exception is the upstream round-trip of
  a FETCH the cache cannot serve: it runs in an `UpstreamFetchTask` (see "FETCH
  sequence"), so a publisher that answers FETCH late or never — a browser
  publisher has no FETCH handler — does not hold the session's later requests
  for the control-message response timeout. Likewise, replies to forwarded
  PUBLISH_NAMESPACE complete in a `PublishNamespaceResponseTask` (see
  `ControlMessageForwarder`), and every forwarded PUBLISH runs in a
  `DownstreamPublishTask` (see "PUBLISH sequence").
- A TRACK_STATUS carrying an AUTHORIZATION TOKEN is handled by the worker
  itself as a token refresh (see "Token refresh" under "Session intake"); one
  without a token goes to `sequences::track_status`.
- An upstream PUBLISH_DONE goes to
  `sequences::upstream_publish_done::UpstreamPublishDone`: it ends the
  matching `ActiveUpstreamSubscription` and stops its ingress. When that was
  the last upstream subscription of its track, the track's downstream
  registrations end with it, handed the received status code and reason (see
  "Egress"); otherwise the remaining publishers keep feeding them. No UNSUBSCRIBE is sent, since the publisher
  already ended the subscription.
- Events for control messages without relay-side logic yet (MAX_REQUEST_ID,
  REQUESTS_BLOCKED, PUBLISH_NAMESPACE_CANCEL, FETCH_CANCEL) are logged in the
  event span and dropped by the worker; they have no `sequences` entry.
- Two relay-internal events exist, both reported by the ingest path (not by
  a peer) and routed to the upstream publisher session's worker:
  - `MalformedTrackDetected(session_id, track_key)`, raised by the reader that
    latched the track. Handled by
    `sequences::malformed_track::MalformedTrackCleanup`: remove every
    `ActiveUpstreamSubscription` of the track, since the cache latch makes the
    whole track malformed whichever publisher's object conflicted, send each
    publisher UNSUBSCRIBE (§2.5 MUST), and stop their ingress via
    `IngressCommand::StopTrack`. Duplicate reports are idempotent (the track
    entry is only found once).
  - `ProtocolViolationDetected { reason }`, raised when a subgroup object
    carries an Object Status draft-14 §10.2.1.1 does not define. The worker
    closes the session with PROTOCOL_VIOLATION (`Session::close_with_error`);
    the resulting `ProtocolViolation` session event then drives the ordinary
    terminal cleanup.
- Terminal events (`Disconnected` / `ProtocolViolation`) trigger
  `cleanup_session` (idempotent) and end the worker. Cleanup: drop the session
  from the repository, then remove it from the pub/sub directory (which stops
  the egress runners of every removed downstream subscription, and ends with
  TRACK_ENDED the downstream subscriptions of every track whose last upstream
  subscription was the session's, see "Egress"), stop the session's ingress,
  forward upstream UNSUBSCRIBE / stop ingress for the upstream subscriptions
  the last downstream subscriber released, and withdraw namespace routes for
  client sessions. Leaving the repository first means an `UpstreamJoinTask`
  adding the session to a track concurrently is either found by the directory
  removal or, checking the repository after adding, finds the session gone
  and removes its subscription again.

### `modules/control_plane/sequences` — one struct per control message
Each sequence owns the relay-side protocol logic for one message
(`publish`, `subscribe`, `subscribe_update`, `fetch`, `track_status`,
`publish_namespace`, `publish_namespace_done`, `subscribe_namespace`,
`unsubscribe`, `unsubscribe_namespace`). Shared collaborators:

- `ControlMessageForwarder` — sends control messages on *other* sessions via
  the repository (e.g. forwarding SUBSCRIBE upstream, PUBLISH_NAMESPACE to
  interested subscribers). `publish_namespace` returns once the message is
  written (`moqt::Publisher::begin_publish_namespace`) and hands the reply to a
  `PublishNamespaceResponseTask`, which only logs it: writing in the caller
  keeps PUBLISH_NAMESPACE ordered before a later PUBLISH_NAMESPACE_DONE for
  the same subscriber. The fan-out includes the originating session when it
  subscribed to a matching prefix (draft-14 §6.1 echoes PUBLISH_NAMESPACE back
  to its sender).
- `InMemoryLocalPubSubDirectory` (`domain/pub_sub_directory.rs`) — the relay's in-memory
  registry of publish/subscribe namespaces, upstream tracks, and downstream
  subscriptions. Each namespace entry carries its sessions' `SessionPeer`:
  client-owned Redis routes are cleaned up when the last *client* leaves, while
  local namespace subscribers get PUBLISH_NAMESPACE_DONE only once no client or
  relay session publishes the namespace. An `UpstreamTrack` holds one
  `ActiveUpstreamSubscription` per publisher session feeding the track
  (draft-14 §8.2) and the count of its downstream subscribers; downstream
  subscriptions refer to the track, not to a publisher. The track lives while
  any upstream subscription feeds it. The last downstream subscriber leaving
  releases its SUBSCRIBE-initiated upstream subscriptions, while
  PUBLISH-initiated ones stay until their publisher ends them. `remove_session` returns everything cleanup needs.
  An `ActiveUpstreamSubscription` keeps the `moqt::Subscription` handle, and
  a release hands that handle back (`ReleasedUpstreamSubscription`), because
  moqt addresses UNSUBSCRIBE / PUBLISH_DONE / FETCH_CANCEL by the
  subscription or fetch handle rather than by its request id.
- `UpstreamCreationSerializer` — per-(namespace, track) async lock. The
  guard removes the track's entry on release unless a waiter still holds the
  mutex, so the map only holds tracks whose upstream creation is in progress.

### SUBSCRIBE sequence (the central flow)
1. **Find-or-create upstream subscription.** Fast path: an
   `UpstreamTrack` already exists in the directory. Miss: take
   the per-track serializer lock, re-check (a sibling may have created it),
   otherwise resolve the publishers and send upstream SUBSCRIBE to every one
   of them at once (draft-14 §8.4). The first SUBSCRIBE_OK starts ingress,
   registers the track and serves this SUBSCRIBE; the still pending requests
   go to an `UpstreamJoinTask` (`sequences/subscribe/upstream_join_task.rs`),
   started once this SUBSCRIBE's downstream subscription is registered. It
   adds each publisher that answers later to the track while the track still
   wants it — someone watches it, and for a relay publisher a client does —
   and unsubscribes it otherwise (the track ended, nobody watches it any more,
   or the publisher already feeds it). A publisher that never answers —
   e.g. a session that died without closing and lingers until its idle
   timeout — therefore delays nobody, and the SUBSCRIBE fails only when every
   publisher refuses it. Concurrent subscribers to the same track produce one
   upstream subscription per publisher.
2. **Publisher resolution** (`UpstreamPublisherResolver`): every local client
   publisher, newest first (session ids grow with time), and, for a SUBSCRIBE
   from a client, every remote relay the route registry lists as publishing the
   namespace (see "Cascading relays"). Resolution dials nothing: each upstream
   SUBSCRIBE task reaches its relay through `InterRelayConnectionManager`
   itself, with a 3 s connect timeout, so an unreachable relay holds back no
   other publisher. A SUBSCRIBE from a relay is served from local client
   publishers only.
3. **Largest Object resolution**: max of the upstream SUBSCRIBE_OK location
   and the local cache's largest location, resolved together with the upstream
   subscription (`get_or_create_upstream_subscription`). The cache is
   consulted even for a fresh upstream: a publisher that rejoined under the
   same track must not make the relay advertise
   `contentExists=false` and replay stale cache from {0,0}.
4. **Downstream registration + egress start**: register the downstream
   subscription — atomically with the upstream subscription's existence, so a
   registration never outlives an upstream a concurrent cleanup already
   removed — which returns the registered subscription's runner stop
   `oneshot::Receiver`. Send
   `EgressCommand::StartReader` carrying it and wait for the runner's
   readiness `oneshot`, then send SUBSCRIBE_OK with the allocated track alias
   and resolved largest location — SUBSCRIBE_OK and egress start always agree.
   If a concurrent cleanup removed the upstream before registration, or the
   registration before the runner reported readiness (e.g. the last other
   subscriber left, or the publisher ended the track), the sequence goes back
   to step 1 and finds or creates the upstream again, up to three attempts;
   only then does it reply SUBSCRIBE_ERROR TRACK_DOES_NOT_EXIST. Another
   session leaving must not fail a SUBSCRIBE that a fresh upstream can serve.

### Publishers joining a received track
draft-14 §8.4 makes a relay subscribe a publisher that announces a namespace
whose tracks it already receives from other upstream sessions. After
answering PUBLISH_NAMESPACE from a client session, `PublishNamespace` sends
SUBSCRIBE to that publisher for every track of the namespace that has a
downstream subscriber and that the publisher does not feed yet, and hands the
requests to an `UpstreamJoinTask`, which adds the publisher once it answers.
A PUBLISH_NAMESPACE from a relay does the same for the publishing relays the
route registry lists, reached over the sessions `InterRelayConnectionManager`
keeps per relay, and only for tracks a client watches (see "Cascading
relays"). A PUBLISH for a track the relay already receives adds its publisher
to the track the same way.

### PUBLISH sequence
1. Add the publisher to the track's `UpstreamTrack` as an
   `ActiveUpstreamSubscription` of origin `Publish` (it survives without
   downstream subscribers), register the PUBLISH for namespace lookups, and
   start ingress.
2. Forward PUBLISH to every local session whose SUBSCRIBE_NAMESPACE prefix
   matches (draft-14 §8.4) and, for a client-origin PUBLISH, to every remote
   relay holding a namespace-subscriber route, dialled via
   `InterRelayConnectionManager`.
3. PUBLISH_OK to the publisher.

Each forward in step 2 is a `DownstreamPublish`
(`control_plane/sequences/downstream_publish.rs`), which treats the forwarded
PUBLISH as a downstream subscription of the track (§5.1: a PUBLISH_OK
establishes a subscription):
- a subscriber that already has a downstream subscription to the track, e.g.
  from the PUBLISH of an earlier publisher of it, gets no second PUBLISH;
- the PUBLISH advertises the Largest Location resolved like a fresh
  SUBSCRIBE (max of the cache and the track's Content Exists), and the egress
  runner starts from the same location;
- on PUBLISH_OK the subscription is registered under the forwarded PUBLISH's
  Request ID with the receiving session's `SessionPeer` — so UNSUBSCRIBE,
  SUBSCRIBE_UPDATE and the split-horizon counts treat it like a SUBSCRIBE —
  and `EgressCommand::StartReader` starts its runner with the parameters of
  the PUBLISH_OK. The runner's acknowledgement signal is already resolved,
  since the PUBLISH_OK has been received;
- a track that ended before the PUBLISH_OK arrived is answered with
  PUBLISH_DONE TRACK_ENDED instead of a registration; a PUBLISH_ERROR or
  timeout registers nothing.

The subscription starts at the Forward State of the PUBLISH_OK (§5.1: the
sender of PUBLISH_OK sets it), and the forwarded PUBLISH waits for that
PUBLISH_OK before any object is sent.
Each forward runs in its own `DownstreamPublishTask`, so the worker answers the
publisher without waiting for any subscriber's PUBLISH_OK, and a subscriber
that answers late or never holds neither the publisher's session nor the other
subscribers' forwards. A publisher that ends the track meanwhile is covered by
the PUBLISH_DONE TRACK_ENDED answer above.

### SUBSCRIBE_NAMESPACE sequence
Register the prefix (and, for the first client subscriber of the prefix, its
Redis route), reply SUBSCRIBE_NAMESPACE_OK, and only then forward what already
matches: PUBLISH_NAMESPACE for published namespaces (reply handled by a
`PublishNamespaceResponseTask`), a `DownstreamPublish` for each published
track (one per publisher session), then PUBLISH_NAMESPACE for remote
namespace-publisher routes. The OK goes first because draft-14 §9.28 forwards
the existing messages once the subscription succeeded; a subscriber that
handles PUBLISH only after its SUBSCRIBE_NAMESPACE_OK would otherwise never
answer the PUBLISH before the request timeout.

### SUBSCRIBE_UPDATE sequence
Forward State, Start Location and End Group are applied; Subscriber Priority
is not. Every downstream registration owns a `watch::Sender<DeliveryControl>`
(`domain/delivery_control.rs`: the Forward State, the Start Location resolved
from the filter and the subscribe-time Largest Location, and the End Group)
next to its runner stop sender. The End Group is kept as the first group past
the range, `None` when open-ended: an AbsoluteRange End Group is the last group
included (§9.7) and is stored plus 1, while the SUBSCRIBE_UPDATE field already
is the end group plus 1 with 0 meaning open-ended (§9.10). A SUBSCRIBE starts
it at Forward 1 whatever the SUBSCRIBE asked for (clients that leave Forward at 0
expect delivery); a forwarded PUBLISH starts it at its PUBLISH_OK's Forward.
SUBSCRIBE_UPDATE applies to the (session, Subscription Request ID)
registration: Forward is taken as sent, the range only narrows (the later
Start Location, the smaller End Group). An update that widens the range is
not closed with the PROTOCOL_VIOLATION §9.10 allows: its widening part is
logged and ignored. An update for no registered subscription is logged and
dropped. The upstream subscription keeps Forward 1 and its own range, so the
cache keeps filling for FETCH.

### FETCH sequence
Resolve the track and object range (Standalone from the message; Relative
Joining from the downstream subscription's start location), reply FETCH_OK,
then delegate to `EgressCommand::StartFetch`, which serves the range from
`TrackCache` over a new uni stream (see "Fetch delivery" under "Egress").

A range the cache cannot serve goes upstream. The worker hands it to
`UpstreamFetchTask` (`control_plane/sequences/fetch/upstream_fetch_task.rs`) and moves on to
the session's next event; the task forwards the FETCH, waits for its
FETCH_OK, replies FETCH_OK (or FETCH_ERROR) downstream and starts
`FetchIngest`, which fills the cache and hands the range to egress. draft-14
§8.4 lets the relay send it to any one publisher, so the task tries them in
turn until one answers FETCH_OK: first the publishers whose upstream
SUBSCRIBE for the track succeeded (known to be alive), newest first; only when
all of them fail are the other publishers of the track resolved, and dialled,
newest first. A FETCH_ERROR or a timeout moves on to the next; only when every
publisher failed is the last error relayed downstream. A
downstream session that disconnects meanwhile only makes the FETCH_OK send
fail; egress drops a `StartFetch` for a departed session.

### TRACK_STATUS sequence
A status query is answered from the relay's own state only. For a track with
an `UpstreamTrack`, TRACK_STATUS_OK carries the newest upstream subscription's
Expires and the Largest Location resolved as on the SUBSCRIBE fast path (the
cache's largest location, else the largest Content Exists of its upstream
subscriptions). Any other track
is answered TRACK_STATUS_ERROR `NOT_SUPPORTED (0x3)`: draft-14 §9.20 lets a
relay without an active subscription forward the request or subscribe
upstream (MAY), and this relay does neither.

### GOAWAY sequence
`sequences::go_away::GoAway`, draft-14 §3.5. A GOAWAY from a client is only
logged: the client is leaving and its state goes with its session. A GOAWAY
from a relay-peer session means that relay is draining and will close the
session, so for every track that session feeds the sequence resolves the
publishers again through `UpstreamPublisherResolver` (local client publishers
and the relays still `active` in the route registry; the draining relay is no
longer active there) and subscribes those not already feeding the track through
`send_upstream_subscribes` / `UpstreamJoinTask`, like a newly announced
publisher. The departing session's own subscriptions end through the ordinary
cleanup when it closes; a track another publisher joined in the meantime keeps
its downstream subscribers.

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
- `token_parameter.rs` — `extract_token(&[AuthorizationToken])`: the first
  AUTHORIZATION TOKEN parameter must be `USE_VALUE` with Token Type `0` and a
  UTF-8 value (the JWT). Other alias types are not supported by design. Used
  for CLIENT_SETUP and for the TRACK_STATUS token refresh.
- `token_verifier.rs` — `TokenVerifier` trait with
  `VerifyError::{Unauthorized, Unavailable}`; the split lets callers map a
  rejected token and an unreachable VTS to different termination codes.
- `token_claims.rs` — `build_verified_token(SignedToken, ClaimPolicy, now)` (`SignedToken.claims` is `auth_token::Claims`):
  the relay, not the VTS, decides what a signed token means. It requires
  `iat` and `exp`, applies the clock leeway, rejects client tokens whose
  `exp - iat` exceeds the maximum ttl (relay tokens are exempt), and checks
  the `publish` / `subscribe` path shape. Time semantics live here so the
  relay's expiry task and its acceptance decision share one clock.
- `vts_token_verifier.rs` — `reqwest` implementation: `POST {AUTH_VTS_URL}`
  with `{"token"}`. The VTS only vouches for the signature and the appId; 200
  carries `{ appId, isRelay, claims }` with the raw JWT payload, which is
  handed to `build_verified_token`. 401 → `Unauthorized`, anything else or a
  transport error → `Unavailable`. 3 s timeout.
- `session_authenticator.rs` — `SessionAuthenticator`, built from
  `AuthConfig` around a `TokenVerifier`; combines the pieces above into the
  CLIENT_SETUP decision described under "Session intake".
- `request_gate.rs` — `authorize_request` / `reject_unauthorized`, the
  per-request gate described under "Event pipeline".
- `token_refresh.rs` — `refresh_token(verifier, current_token, tokens)`: the
  decision behind the TRACK_STATUS token refresh described under "Session
  intake"; returns the new `VerifiedToken` or the TRACK_STATUS_ERROR code and
  reason.
- `session_expiry_task.rs` — `SessionExpiryTask` (owns its `JoinHandle`,
  aborted on drop) that sleeps until `expires_at` and closes the session via a
  `Weak<dyn Session>` so a departed session is a no-op.

## Data plane

### Shared state
- `TrackCacheStore` — `DashMap<TrackKey, Arc<TrackCache>>`.

### Ingress (`modules/data_plane/ingress`)
`IngressCoordinator` consumes `IngressCommand::{Start, StopTrack}`:

- On `Start`, it obtains the upstream session's `Subscriber`, creates the data
  receiver (cancellable via a `watch` stop channel per track and publisher), and hands it to
  `TrackIngestTask` as an `IngestSource` (subgroup-stream factory or datagram
  receiver).
- `TrackIngestTask` (`track_ingest_task.rs`) runs one task per track,
  publisher and source kind: `accept_streams` (`stream_reader.rs`) spawns a `read_stream`
  per accepted subgroup stream, `read_datagrams` (`datagram_reader.rs`) reads
  the datagram receiver. Every reader of one publisher's track shares one `TrackIngest`
  (track key, publisher, cache, session-event sender, stop receiver).
  **Every publisher is ingested** (draft-14 §8.2): all publishers of a track
  insert into the same `TrackCache`, whose §8.1 duplicate handling
  deduplicates them. A second `Start` for a publisher already ingesting the
  track is ignored, and a publisher's `Stop` tears down only its own readers.
  Stream readers outlive their accept loop until their stop sender is
  signalled or dropped.
- Readers convert every object moqt hands them (`moqt::object::Object`, ids
  already resolved) into a canonical `CachedObject` and insert it into
  `TrackCache`. A subgroup header (`SubgroupStart`) is not cached: the reader
  keeps its group id, subgroup id and priority as the per-stream context, opens the
  subgroup in the cache (`open_subgroup`, returning an `OpenSubgroupGuard`),
  which also announces the `SubgroupKey` to egress. Only a FIN or an End of Group / End of Track
  object `finish`es the guard; every other end (RESET_STREAM, stop, decode
  error, task abort) drops it, which marks the subgroup aborted: its group is
  not declared complete (draft-14 §10.4.2) and egress resets, rather than
  FINs, the downstream stream (§10.4.3). Publishers of one track send the
  same objects (§2.1), so when several publishers deliver a stream subgroup, the
  FIN of a stream that delivered it from the earliest object any of its
  streams started at completes it for all of them, and later streams for it
  only add duplicates. A stream that joined later started mid-subgroup and
  proves nothing below its first object, so its FIN only closes that stream;
  the subgroup is aborted when its last stream ends without a completing FIN.
  A datagram group stays open until every publisher has moved on from it, and
  is aborted only when none of them moved on cleanly. A later live stream for an aborted,
  no-longer-open subgroup (e.g. a new upstream subscription after the
  previous one was cancelled) reopens it as the next `SubgroupRun`: the abort
  mark is cleared so the new stream's FIN or reset decides the tail again, and
  the run's first object id lies past every object cached for the subgroup,
  so the objects below it belong to the superseded runs. When the header
  says the subgroup ends its group (`SubgroupStart::ends_group`), a clean FIN
  inserts an EndOfGroup status object at `last_id + 1`, so the signal
  survives header regeneration as data. A protocol violation moqt reports on
  the stream (an unknown Object Status) terminates the publisher session.
- `FetchIngest` inserts each FETCH object one-to-one (`CachedObject::from_object`
  via `TrackCache::insert`, which registers no knowledge); no header synthesis and
  no per-subgroup delta state.
- The cache's sticky §2.5 malformed latch is set by an ingest reader: by the
  insert that conflicts, or when moqt returns `MalformedTrackError` because one
  publisher's upstream subscription switched between subgroup streams and
  datagrams (`accept_streams`, `read_datagrams`). That reader is always
  present to report `MalformedTrackDetected` into the event pipeline — no
  standing watcher is needed. Two publishers that use different forwarding
  preferences for one track are not detected unless their objects conflict. Downstream, `EgressRunner` watches the same
  latch and terminates subscriptions with PUBLISH_DONE(MALFORMED_TRACK);
  `FetchIngest` bails on the latch and sends upstream FETCH_CANCEL for its
  own fetch.

### Cache (`modules/data_plane/cache`)
- `CachedObject` (`cached_object.rs`) is the draft-14 §10.2.1 canonical object:
  the `moqt::object::Object` moqt returned (location, forwarding preference,
  publisher priority, status, extension headers, payload) plus its own
  `received_at`, and `expires_at` when the subscription or fetch it arrived on
  carried MAX_CACHE_DURATION. Egress hands the `Object` back to moqt, which picks the
  header, datagram and FETCH encodings, computes the deltas, and gives a
  fetched datagram object its Object ID as Subgroup ID (§10.4.4).
  `conflicts_with` implements §8.1:
  differing forwarding preference, subgroup, priority or payload, or a status
  move between Normal/EndOfGroup/EndOfTrack, is a Malformed conflict;
  extension changes and Does Not Exist transitions are tolerated duplicates.
  A datagram's `ends_group` is not compared. When a live datagram meets a
  fetched copy that arrived first, the cache takes the forwarding preference
  and `ends_group` from the datagram.
- `TrackCache` (`track_cache.rs` + `track_cache/{ledger,open_subgroup}.rs`) is one
  track-level ledger behind a `std::sync::RwLock` that is never held across an
  await: `objects: BTreeMap<Location, Arc<CachedObject>>` (stream and datagram
  objects together, so identity is the key, never an entry), `open_subgroups`
  (per subgroup, the number of live streams delivering it and the epoch of
  that opening, so a stream that outlived the subgroup's close cannot close a
  later opening — the only non-data state besides the aborted / finished
  marks), and `KnownRanges` (§9.2.1.3 / §9.16 unknown-status
  semantics). One `Notify` per track wakes every waiter on insert, open and
  close; waiters re-check the ledger under a single read guard, so there is no
  check-order race between "object present" and "subgroup closed".
- Subgroup-opened channel: each `TrackCache` owns a
  `broadcast::Sender<SubgroupRun>` (capacity 256; key, generation, first object
  id). `open_subgroup` sends the run
  after the subgroup is in the ledger, and egress schedulers subscribe through
  `subscribe_subgroup_opened`, so a subgroup is either announced to a
  subscribed scheduler or already visible to its cache scan. The channel lives
  and is evicted with the cache it belongs to.
- Knowledge: a live subgroup insert registers only the received position
  (§10.4.2: ids skipped by a non-zero delta cannot be inferred). Each open
  group keeps the largest object id live ingest has seen (`LiveGroup`), and
  closing the last open stream subgroup registers only the tail after it, so
  an evicted position is never re-claimed as known (§9.2.1.3 "their state
  becomes unknown") and a skipped id stays undecided. Fetch fills register
  their requested range only at `Fetch::End` (guarded by the eviction
  generation counter); datagram objects register nothing.
- `next_subgroup_object_or_wait(key, generation, from)` (live egress) returns the next object of that
  subgroup, `Finished` once it closed cleanly, or `Aborted` once it closed without
  a FIN; a subgroup that was never opened (fetch-fill only) therefore never blocks.
  For a superseded generation it returns only the objects below the next run's
  first object id and then `Aborted`, so a downstream stream never continues
  into a reopened run (§10.4.3: a relay that cannot prove an object is the next
  one resets the stream and opens a new one).
- `FetchCursor::new(cache, start, end, group_order)` (`track_cache/fetch_cursor.rs`)
  walks `[start, end)` in delivery order (Descending reverses the group
  list) and yields one object per `next`, reading positions inside knowledge
  without waiting and waiting past the frontier only while some subgroup of
  the group is open. A subgroup of the group reopening after the cursor entered
  it counts as `Aborted`, since the reopened run cannot restore the objects
  lost with the reset. `Aborted` from such a wait surfaces as
  `FetchInterrupted::Incomplete`, the malformed latch as
  `FetchInterrupted::Malformed`.
- Eviction job (`eviction_job.rs`): every `RELAY_CACHE_EVICT_INTERVAL_SECS`
  (5 s) drop objects older than `RELAY_CACHE_TTL_SECS` (60 s) and release
  knowledge exactly for the removed locations. While live ingest runs, the
  object at the track's largest location is kept past the TTL: a late
  subscriber is answered Content Exists from the cache (§9.8) and fetches that
  object, so a sparse track such as a catalog stays joinable; a `TrackCache` entry is removed
  from the store only when it is empty and `Arc::strong_count == 1`, i.e. no
  ingress/egress holds it — avoiding races with new joiners.
- MAX_CACHE_DURATION (draft-14 §9.2.1.3): ingress stamps every object with
  `expires_at` = this relay's receipt time + the value its upstream
  SUBSCRIBE_OK, PUBLISH or FETCH_OK carried. From that instant
  the object is never handed out: a live read skips it, a FETCH served from
  cache ends with `FetchInterrupted::Incomplete` (the stream is reset), and
  `resolve_fetch_range` answers `NotCovered` for a range that holds one, so the
  FETCH goes upstream. Eviction removes it at `expires_at` whether that is
  before or after the relay TTL, and the newest-object exception does not
  extend it. Objects received without the value keep the relay TTL.
  Downstream, the relay sends the value it received unchanged:
  `UpstreamTrack::max_cache_duration` (the newest publisher's) goes into the
  SUBSCRIBE_OK, the forwarded PUBLISH and a FETCH_OK served from cache, and an
  upstream FETCH_OK's value goes into the relayed FETCH_OK. Each relay counts
  from its own receipt, so in a cascade an object can stay cached up to N ms
  per hop. A value above `RETENTION_LIMIT` (1 h) is cut to it; caching is
  optional (§8.1), so keeping an object for less is allowed.
- Disk cache (`disk_cache.rs`, on when `RELAY_DISK_CACHE_DIR` is set): the
  relay a client publisher sends to keeps the payloads for up to
  `RETENTION_LIMIT`; a downstream relay keeps its memory cache only and
  forwards a FETCH that misses it upstream. The ingress coordinator and
  `FetchIngest` stamp `CacheStamp.persist` when the upstream session is
  `SessionPeer::Client`.
  - Write: `TrackCache` hands each newly inserted persisted object with a
    payload to `DiskSink::write`, a `try_send` to `DiskCacheTask`, the only
    task that touches the directory. It appends the payload to the track's
    current segment file (`<n>.seg`, a new one every 60 s) and reports the
    location back with `TrackCache::record_stored`, which sets the object's
    `stored`. A full queue or a failed write leaves the object in memory
    only, and it leaves at the memory TTL like any other object.
  - Swap: eviction past the memory TTL empties the payload of an object with
    `stored`, so the index (location, status, knowledge) stays in memory and
    `CachedObject::load` reads the bytes back for egress. A persisted status
    object, which has no payload, stays as it is.
  - Removal: a disk-backed object counts as expired (`is_expired`) once it is
    `RETENTION_LIMIT` old or its segment is deleted, exactly like one past
    MAX_CACHE_DURATION: it is not handed out, a FETCH over it goes upstream,
    and the next eviction drops it with its knowledge.
    `DiskCacheTask` deletes a segment once its last object is past
    `retained_until`, and deletes the oldest segments of any track while the
    total exceeds `RELAY_DISK_CACHE_BYTES`. A deleted segment is flagged
    before its file goes; a read already in flight fails, which resets the
    FETCH stream or makes a live read skip the object.
  - End of a broadcast: a FETCH after it need not be served. Session cleanup
    and an upstream PUBLISH_DONE call `TrackCacheStore::publisher_left` for
    a track no publisher serves any more; once 10 s pass without an ingest
    beginning again, eviction drops the track's persisted objects with their
    knowledge and sends `PurgeTrack`, which deletes the track's segments. A
    publisher that reconnects within that time keeps them. A
    subscriber-driven upstream release is not a publisher leaving and purges
    nothing.
  - Nothing survives a restart: the index lives in memory, and startup deletes
    the `*.seg` files in the directory.

### Egress (`modules/data_plane/egress`)
`EgressCoordinator` consumes `StartReader` / `StartFetch`. There is no stop
command: each registered downstream subscription in the pub/sub directory owns the
`oneshot::Sender<PublishDoneReason>` whose receiver `EgressRunner::run` selects on (biased,
before its delivery), so the runner lives exactly as long as the registration.
Whoever removes the registration — UNSUBSCRIBE, the subscriber's or the publisher's session
cleanup, an upstream PUBLISH_DONE — stops the runner, and a registration removed before the
coordinator got to `StartReader` yields a runner that never runs; the order in which different
session workers touch the registration and the coordinator cannot leak a runner. Removal
drops the sender when the subscriber ended the subscription (draft-14 §5.1: the publisher
may destroy its state). Removal because the track's last upstream ended — its publisher's session went
away (TRACK_ENDED) or it sent PUBLISH_DONE (its status code) — sends a `PublishDoneReason`
instead, and the runner answers it with PUBLISH_DONE carrying that status and the number
of streams it opened, after its senders stopped (§9.12: no state is destroyed without
PUBLISH_DONE). A malformed track ends the runner the same way with MALFORMED_TRACK. The
runner sends PUBLISH_DONE only once the subscribe sequence signals that SUBSCRIBE_OK went
out (`subscribe_ok_receiver`), so it never precedes SUBSCRIBE_OK; a runner stopped before
readiness drops it, the SUBSCRIBE fails with SUBSCRIBE_ERROR and no PUBLISH_DONE is sent. A
forwarded PUBLISH passes an already resolved signal: its PUBLISH_OK has been received. The
coordinator keeps the runner tasks in a `JoinSet`, reaping each one as it
finishes (including runners that end on their own, e.g. on a malformed track)
and aborting the rest when it shuts down. `EgressRunner` splits into:

- `EgressScheduler` — listens on the cache's subgroup-opened channel and the cache,
  takes the delivery start from the registration's `DeliveryControl` (the
  Start Location of the draft-14 filter type, `NextGroupStart`,
  `LargestObject`, `AbsoluteStart` or `AbsoluteRange`; an absolute start at or
  below Largest is clamped to Largest+1), and emits one `GroupSendTask` per
  `SubgroupKey` and run generation: a reopened run is scheduled again from
  the later of the key's previous task start and the run's first object id,
  while further opens of an already scheduled run are skipped. It subscribes to open events first and then schedules every
  cached group at or after the start, so a group that ingress opened and
  closed before the scheduler existed is still delivered. While the
  registration's Forward State is 0 it drops open events, and a runner that
  starts at Forward 0 skips that cache scan, so no subgroup opened meanwhile is
  ever sent; streams already scheduled run to their end, and
  after Forward returns to 1 delivery resumes with the next subgroup opened. The start is a
  lower bound in both paths: group ids may begin anywhere and skip values
  (§2.3.1), so the first delivered group is the first one at or above the
  start, not the start group itself. Groups at or past the End Group are never
  scheduled, and an open event reads the current `DeliveryControl`, so a
  SUBSCRIBE_UPDATE's later Start Location or smaller End Group applies to every
  subgroup opened after it.
- `GroupSender` — one task per subgroup: waits for the first object to send,
  only then opens the downstream uni stream (a subgroup that closes empty
  opens nothing), regenerates the SUBGROUP_HEADER from that object's canonical
  properties (explicit subgroup id, priority; extensions always declared
  present so no object can lose its extension headers), and streams objects
  until `next_subgroup_object_or_wait` reports the subgroup finished (FIN
  downstream) or aborted (RESET_STREAM downstream, INTERNAL_ERROR). Each task
  also watches the registration's `DeliveryControl`: when a SUBSCRIBE_UPDATE
  moves the range past its group (End Group at or below it, Start Location
  beyond it), the open stream is reset with CANCELLED (draft-14 §10.4.3) and a
  task that has not opened its stream yet ends without one. Groups still in
  the range are delivered to their end. Datagram groups are
  re-emitted with the downstream track alias, and stop at the same range
  change.
  Each stream is opened with a `moqt::object::StreamPriority` (draft-14 §7.2):
  the subscription's subscriber priority, the first object's publisher
  priority, the subscription's group order with a per-subscription
  `GroupSequence` (incremented for every newer group id the sender sees,
  shared by the subgroups of one group), and the subgroup id.
  `ConcreteStreamSenderFactory::next(priority)` applies it before the header
  is written. The sequence, not the group id, feeds the rank because group
  ids may be wall-clock timestamps; assigning it at open time means a still
  buffered tail of group N keeps outranking group N+1's keyframe without
  re-prioritizing streams already queued in the transport.
- Fetch delivery (`fetch_delivery.rs`) — one task per FETCH served from the
  cache: it opens the fetch stream, drives the `FetchCursor` and writes each
  object as soon as the cursor yields it, so objects of already-closed groups
  reach the subscriber while a later group in the range is still open. The
  stream is FIN'd when the cursor is exhausted and reset with
  MALFORMED_TRACK / INTERNAL_ERROR when the cursor reports `Malformed` /
  `Incomplete`.

## Cascading relays (`modules/cascading`)

- `RelayRouteRegistry` trait: `NoopRelayRouteRegistry` (single-relay, no
  `REDIS_URL`) or `RedisRelayRouteRegistry` (relay info hash and
  namespace-publisher / namespace-subscriber routes with a 15 s TTL). The
  registry keeps the routes it registered in memory, and its 5 s heartbeat
  rewrites the relay info hash and re-registers any owned route whose entry is
  missing, so state that expired while the relay, its host or Redis was
  stalled is restored instead of staying lost until restart.
- The relay info hash `relay:<RELAY_ID>` carries two addresses: `host:port`
  (`RELAY_ADVERTISE_HOST`, `RELAY_INNER_PORT`) is what other relays dial, and
  `public_host` (`RELAY_PUBLIC_HOST`) is what clients connect to. Its `status`
  is `active` until `mark_draining` turns it into `draining` for the rest of
  the process (the heartbeat keeps writing it): the route lookups and the Lua
  scripts only return `active` relays, so a draining relay receives no new
  cascaded subscriptions. The heartbeat also writes `client_sessions`, the
  relay's current number of client sessions (`SessionRepository`'s watch,
  handed to the registry when `RelayServer` connects it), and `find_migration_target` names
  the `public_host` of the other `active` relay with the fewest of them for
  the GOAWAY a draining relay sends (see "Shutdown").
- Only **client-origin** namespaces register routes: `PublishNamespace`
  registers the publisher route and notifies remote subscriber relays;
  `SubscribeNamespace` registers the subscriber route when the first client
  subscriber for a prefix appears. Any number of relays may hold the publisher
  route of one namespace (draft-14 §8.2), and the notification reaches every
  subscriber relay.
- **Watched namespaces**: a relay receiving a track for a client must learn
  about publishers of its namespace announcing later, on any relay. Before
  resolving the publishers of a track a client asked for, the relay registers
  a watched-namespace route, which `find_namespace_subscribers` returns like a
  namespace-subscriber route, so the later PUBLISH_NAMESPACE is forwarded to
  it (see "Publishers joining a received track").
  `WatchedNamespaceRoutes` (`cascading/watched_namespace_routes.rs`) holds
  these routes, and `WatchedNamespaceJob` reconciles them every 5 s with the
  namespaces the directory has a client watching, dropping a route after two
  passes in a row without one.
- **Split horizon**: relay-to-relay subscriptions only carry what clients
  need. A SUBSCRIBE or FETCH from a relay is resolved to local client
  publishers only, a relay's upstream subscription to another relay is
  released when the last client subscriber of the track leaves, and a relay
  subscriber left on a track no upstream subscription feeds any more ends with
  TRACK_ENDED. Two relays both publishing and both watching a track therefore
  subscribe each other, yet the pair never keeps the track alive once their
  clients are gone.
- `InterRelayConnectionManager` lazily dials the remote relay's inner endpoint
  over raw QUIC (`moqt::QUIC`, certificate verification disabled), presenting
  this relay's own JWT (`AUTH_RELAY_TOKEN`) in CLIENT_SETUP, and registers the
  session as `SessionPeer::Relay` with `VerifiedToken::full_access()`, reusing
  it afterwards. From then on the remote relay behaves like any upstream
  publisher session.
- A client's PUBLISH reaches every remote relay `find_namespace_subscribers`
  returns (namespace-subscriber and watched-namespace routes) as a
  `DownstreamPublish` on the dialled session (see "PUBLISH sequence"). The
  remote relay adds the dialling session to the track as a relay publisher of
  origin `Publish` and forwards the PUBLISH to its own namespace subscribers
  the same way. Objects flow because the forward is a downstream subscription
  of the origin relay's track, and the remote relay's upstream subscription
  ends with the PUBLISH_DONE the origin relay sends once its track ends; the
  inter-relay session itself stays up.

## Observability (`modules/observability`)

The relay publishes its own statistics as an ordinary MoQT track,
`observability/<RELAY_ID>` / `network_stats` (format: `crates/relay-stats`),
so cache, fan-out, FETCH and authorization need no separate path.

- `StatsPublishTask` dials the relay's own inner endpoint over loopback QUIC
  with `AUTH_RELAY_TOKEN`, sends PUBLISH, and writes one group per second
  whose Group ID is the snapshot's Unix time in milliseconds (never below
  the next id, so a clock step back cannot reuse a location). A failed
  session is retried every 5 s. The loopback session is a `SessionPeer::Relay`
  with full access; the snapshot reports it as `stats_publisher`, and a session
  subscribed to the relay's own stats track as `stats_subscriber`.
- `StatsCollector` reads, without holding any lock across an await:
  `SessionRepository::session_states` (peer, `app_id`, `Session::transport_stats`
  and `transport_addresses`), `InterRelayConnectionManager::dialed_relay_ids`
  (the relay at the other end of each session this relay dialed; an accepted
  inter-relay session is matched by the consumer through the relays' known
  addresses, see `docs/architecture/observability/architecture.md`),
  `InMemoryLocalPubSubDirectory::active_upstream_tracks` and
  `downstream_subscription_states`, each track cache's `IngressStats`, and
  `TrackCacheStore::occupancy`. RSS comes from `/proc/self/status` and is
  absent elsewhere. `active_upstream_tracks` lists every publisher feeding a
  track, so a track with several publishers appears once per publisher, each
  entry carrying the track cache's counters, and a subscription reports the
  track's newest publisher.
- Counters are cumulative. `IngressStats` lives in `TrackCache` and counts
  every object inserted (live or fetch fill), the subgroups closed without
  `finish`, and the longest gap between live inserts; reading it resets only
  that gap. `DeliveryStats` belongs to the downstream registration (it also
  supplies PUBLISH_DONE's stream count) and counts the streams, objects,
  bytes and resets egress sent, plus the `received_at` of the newest object
  sent, from which the collector derives how far the subscriber trails the
  newest object the track received.
- QUIC statistics are per connection and describe what the relay sent, so a
  client's uplink shows no loss or cwnd; the received flow-control and
  reset frame counts are its uplink signals.

## Key invariants

- **Reader never awaits**: the `EventHandler` reader only routes; all awaiting
  happens in per-session workers. Cross-session deadlock is structurally
  impossible; per-session ordering is FIFO, except that an upstream FETCH,
  the replies to forwarded PUBLISH_NAMESPACE messages and forwarded PUBLISHes
  complete in their own tasks after the events that followed them.
- **Worker lifetime is the session's**: a session worker exists from the
  session's registration until its terminal event; events arriving outside
  that window are dropped by the reader.
- **One upstream subscription per track and publisher**: creation is
  serialized by the `UpstreamCreationSerializer` per-track lock with a
  double-check, and a publisher answering after the track already holds its
  subscription is unsubscribed again.
- **SUBSCRIBE_OK matches egress**: the largest location advertised downstream
  is the same value the egress scheduler starts from.
- **Start Location is a lower bound**: egress delivers the first group at or
  above the start whether it arrives as an open event or is already cached;
  neither path requires the start group id itself to exist.
- **Every publisher is ingested**: one reader set per track and publisher,
  all inserting into the track's single cache; a publisher's stop ends only
  its own readers, and only a FIN covering a stream subgroup from its
  earliest received object completes it.
- **Egress runner lifetime is the downstream registration's**: a runner stops
  when its registered downstream subscription is removed, regardless of which
  session worker removes it or when; a downstream subscription is only ever
  registered while its upstream track exists.
- **An ended upstream is announced downstream**: every downstream subscription
  removed because its track's last upstream subscription ended receives PUBLISH_DONE, always
  after its SUBSCRIBE_OK. An upstream PUBLISH_DONE on a PUBLISH-initiated track
  also unregisters that PUBLISH, so the ended track is no longer resolved.
- **A forwarded PUBLISH is a downstream subscription**: every PUBLISH the
  relay sends and the receiver accepts is registered and served by an egress
  runner like a SUBSCRIBE, so it carries objects and ends with PUBLISH_DONE.
- **Cache identity is the key**: a cached object is self-contained (§8.1 "MUST
  store all properties"); nothing in the cache refers to an entry by handle,
  so eviction can never orphan a header or resurrect a partial entry.
- **Live state is bounded**: `open_subgroups` is the only non-data cache state,
  it is held by RAII guards in the readers, and live ingest always closes what
  it opened — so every `*_or_wait` is bounded.
- **Ledger lock is never held across an await**.
- **Egress streams are prioritized at open**: a downstream subgroup stream's
  transport priority is fixed before its header (§7.2 order: subscriber
  priority, publisher priority, group order, subgroup id); the control stream
  keeps the transport default and so outranks all data streams.
- **Knowledge follows objects**: evicting an object releases the knowledge at
  exactly that location.
- **Cache lifetime**: a track cache lives while referenced or until TTL
  eviction empties it with `strong_count == 1`.
- **Client-owned routes**: Redis namespace routes are registered/withdrawn
  only for client-origin sessions, and watched-namespace routes only while a
  client watches the namespace; relay-learned namespaces are purged locally
  when the last client subscriber for the prefix leaves.
- **Relay subscriptions serve clients**: an upstream subscription to another
  relay only exists while a client of this relay watches the track, and a
  relay's SUBSCRIBE is never forwarded to a third relay.
- **A draining relay is left by its clients, not by the relay**: after
  GOAWAY the relay only waits; it closes a client session itself only at the
  drain timeout, with `GOAWAY_TIMEOUT`.

## Testing conventions
Unit tests are colocated (`#[cfg(test)]`) and pin structural invariants —
e.g. reader/worker non-blocking and terminal-event handling in
`control_plane/event_handler.rs`, largest-location resolution in `control_plane/sequences/subscribe.rs`,
eviction refcount rules in `data_plane/cache/store.rs`. Shared mocks and the
data-plane `RelayHarness` live in `test_support/`. Multi-process behaviour
(cascading relays, cache eviction, fetch, multiple publishers, dedup) lives in
the workspace-level `tests/*-e2e` suites, each driven by its `run.sh`.
