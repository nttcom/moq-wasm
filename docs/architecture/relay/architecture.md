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
| `domain/` | Relay vocabulary shared by every layer: `TrackKey`, `SessionId`, `SessionPeer`, `DeliveryControl`, `DeliveryStats`, `NewGroupRequestGate`, `PublishDoneReason`, the egress runner's `DownstreamRunnerSignals` (error codes are `moqt`'s typed enums) |
| `directory/` | The relay's registry: `InMemoryLocalPubSubDirectory` (namespace tables, tracks and the consumer index) and `track.rs` with `Track`, its `Producer`s (each owning the `IngestTask` reading its publisher) and `Consumer`s (each owning the signals its egress runner lives by) |
| `auth/` | Token verification, claims, per-request authorization, token refresh, session expiry |
| `control_plane/` | Control-message processing: `SessionWorkers` and the per-session `SessionWorker`, one `sequences/*` struct per message, `ControlMessageForwarder`, `UpstreamPublisherResolver`, the `PendingProducerTask` behind every upstream SUBSCRIBE |
| `data_plane/` | Objects in flight: `ingress/` (`IngestTask`), `cache/`, `egress/` (`EgressTask`, `FetchDeliveryTask`), and the data-plane integration tests |
| `cascading/` | Relay-to-relay: `RelayRouteRegistry` (Redis / no-op) and `InterRelayConnectionManager` |
| `observability/` | `StatsCollector` (snapshot of sessions, tracks, subscriptions and process memory) and `StatsExportTask`, which sends it once per second as an OTLP log record, and its process values as gauges |
| `test_support/` | `cfg(test)` mocks, the data-plane `RelayHarness` and directory fixtures shared across modules |

Dependencies point downwards in this order: `control_plane` → `cascading` →
`directory` → `data_plane` → `session`, `auth` → `domain`. `observability` only reads
`session`, `directory`, `data_plane` and `domain` state. `auth` and `session` reference
each other only through `VerifiedToken` / `SessionExpiryTask`.

## Startup path

`main.rs`:
1. `init_logging` (tracing + OpenTelemetry OTLP export, set up by `crates/telemetry`).
2. Generate self-signed certs under `RELAY_CERT_DIR` (default `services/relay/keys/`) if missing.
3. `RelayConfig::from_env()` — `RELAY_ID`, `RELAY_ADVERTISE_HOST`,
   `RELAY_PUBLIC_HOST` (the `host[:port]` clients connect to, default
   `localhost:RELAY_PORT`), `RELAY_PORT` (default 4433), `RELAY_INNER_PORT`
   (default port+1), `RELAY_INNER_CA_FILE` (the certificates trusted when
   dialing another relay, otherwise the OS roots; see "Cascading"),
   `REDIS_URL` (optional), and the authentication settings
   (`AuthConfig`):
   `AUTH_VTS_URL` and `AUTH_RELAY_TOKEN` are both required; startup fails
   without them. There is no unauthenticated mode: a relay that should serve
   only anonymous clients still needs a VTS to reject the tokens it cannot
   verify. `AUTH_MAX_TOKEN_TTL_SECONDS` (default 86400) and
   `AUTH_CLOCK_LEEWAY_SECONDS` (default 60) form the `ClaimPolicy` the relay
   applies to token times.
   `RELAY_MOQT_VERSIONS` (`moqt_versions`, default `draft-18,draft-14`)
   lists the drafts both transports accept and the relay offers when it
   dials an inner transport, most preferred first; `draft-14` keeps the
   relay on draft-14. See "Relaying across versions".
   `CacheConfig` (`RELAY_CACHE_TTL_SECS`, `RELAY_CACHE_EVICT_INTERVAL_SECS`),
   the disk cache settings and `MOQT_FETCH_FILL_TIMEOUT_SECS` reach the
   eviction job, the disk cache task and FETCH ingest through
   `DataPlaneConfig`; the data plane reads no environment variable itself.
4. `RelayServer::new_with_config(...)` then:
   - `spawn_client_transport::<moqt::NATIVE>(port)` — client-facing endpoint
     accepting both WebTransport and raw QUIC on one port, for the drafts in
     `moqt_versions`.
   - `spawn_inner_transport::<moqt::QUIC>(inner_port)` — inter-relay endpoint
     for the same drafts. `InterRelayConnectionManager` offers them when it
     dials an inner endpoint, so two relays that both enable draft-18 cascade
     in draft-18, and one that does not is reached in draft-14.
   - `spawn_stats_exporter(relay_id, event_logger, process_gauges)`, with OTLP on — see "Observability".

`RelayServer` (in `relay_server/`) wires two long-lived pieces:

- `SessionRepository` (shared `Arc<Mutex<_>>`) — one `SessionEntry` per
  session: the session, its span, `SessionPeer`, `VerifiedToken`, expiry task,
  remote-address watch task (see "Observability"), the sender of the session's mailbox (see "Event pipeline") and the
  namespaces announced on it (see "SUBSCRIBE_NAMESPACE sequence"). The
  repository announces every added session on the `SessionAdded` channel
  `RelayServer` creates with it.
- `RelayRuntime` — constructs the shared data-plane state (`TrackCacheStore`),
  the directory, `InterRelayConnectionManager`, `UpstreamPublisherResolver`,
  `SessionWorkers` (consuming the `SessionAdded` channel, with the
  `WorkerDeps` every session worker clones) and the cache-eviction job.

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
connection attempt, its transport handshake still to run, to a per-connection task owned by `SessionIntake`:

1. await the `moqt::server::Accepting` future → `Handshake` (CLIENT_SETUP received,
   SERVER_SETUP not yet sent);
2. `SessionAuthenticator::authenticate(authorization_tokens, accepted_peer)`
   extracts the JWT from the CLIENT_SETUP tokens (`Handshake::authorization_tokens()`), calls the `TokenVerifier`, and checks that `is_relay` matches the
   endpoint (client port ⇔ `false`, inner port ⇔ `true`). A client that presents no token on the client endpoint is
   accepted with `VerifiedToken::anonymous()` (scope `anon/**`); a missing
   token on the inter-relay endpoint is rejected. Other failures call
   `Handshake::reject` with `UNAUTHORIZED` (rejected token, endpoint mismatch)
   or `INTERNAL_ERROR` (VTS unreachable);
3. `Handshake::accept(&session_span)` sends SERVER_SETUP. Steps 2 and 3 run
   under a `relay.session.setup` span (`session_id`, `session_peer`,
   `app_id`) that continues the trace of the client's `moqt.session.connect`
   when CLIENT_SETUP carries one (`Handshake::trace_context()`), roots its own
   otherwise, and links to `relay.session`. `relay.vts.verify` is its child.
   The session's tasks start under `relay.session`, so the setup span closes
   once SERVER_SETUP is sent. `CLIENT_SETUP received` is logged before the
   span exists;
4. the session is boxed as `dyn session::Session` and added to
   `SessionRepository` as a `NewSession` carrying its `SessionPeer` (`Client`
   or `Relay` — the endpoint it arrived on) and its
   `VerifiedToken`, which later requests are authorized against. A relay
   token's `relay_id` is recorded on the `relay.session` span as
   `peer_relay_id`, and `relay.subscribe` / `relay.fetch` carry it too, so
   a request from another relay can be matched to that relay's record of
   sending it: a SUBSCRIBE or FETCH that goes upstream to a relay records
   that relay's id and the upstream request's id on the request's span as
   `upstream_relay_id` / `upstream_request_id`
   (`sequences::record_upstream_relay`; for a track with several relay
   publishers, the newest one's);
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
`VerifiedToken` and restarts its `SessionExpiryTask` at the new `exp`, and
TRACK_STATUS_OK is sent with Track Alias 0 and Content Exists false — it
reports no track status. The worker keeps no copy of the token: it reads the
entry's token for each request, so the next request sees the new one. On
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
(GOAWAY, PUBLISH_DONE, …) as they are. The same module holds
`SessionRepository` and `SessionEvent` (`session_event.rs`), the relay-internal
events addressed to one session's worker, so everything the relay knows about
a session lives in one place.

### Event pipeline

```
SessionRepository::add ──SessionAdded { session_id, mailbox_receiver }──► SessionWorkers (one task)
                                                                              │ spawns, reaps
                                                                              ▼
moqt session ──receive_moqt_session_event()──────────────────────► session worker (one task per session)
session mailbox (unbounded, sender in SessionEntry) ──SessionEvent──►   │  select!, FIFO per source
                                                                        ▼
                                                     sequences::{PublishNamespace, Subscribe, Fetch, …}.handle(...)
```

- `SessionRepository::add` inserts the entry with a fresh mailbox and then
  sends `SessionAdded` with the mailbox's receiver; `SessionWorkers`
  (`control_plane/session_workers.rs`) spawns the session's worker for it
  and reaps workers as they exit. The worker reads its session's `moqt` events
  itself (`Session::receive_moqt_session_event`) and `select!`s them with the
  mailbox, mailbox first; each source is FIFO, their interleaving is not
  ordered. The worker holds the session id, its span and `WorkerDeps`;
  everything else about the session (the token, the peer) it reads from the
  repository entry when a request needs it. A relay-internal event (`SessionEvent`: `MalformedTrackDetected`,
  `ProtocolViolationDetected`, `GoAwaySent`, `DownstreamSubscriptionEnded`)
  is sent to the target session's mailbox, whose sender the sender captured
  when it was created (ingress hands the publisher's mailbox to its readers,
  egress keeps the subscriber's for its runner) or looks up in the repository
  (drain, FETCH ingest). A session that already left has no mailbox, so a
  late event for it (e.g. `MalformedTrackDetected` after the publisher
  disconnected) is dropped at the sender and never resurrects a worker.
- A `receive_moqt_session_event` error (the session's event channel closed
  without `Disconnected`) is treated like `Disconnected`: the worker runs the
  terminal cleanup and exits.
- Each session worker opens the event's span (`relay.subscribe`,
  `relay.publish`, …), the one place that lists the event's fields, and logs
  `Received session event` at debug in it; the sequence runs in that span. A request's
  span roots its own trace, unless the request carries `TRACE_CONTEXT`: then
  it continues the client's trace as a child of the client's request span,
  and a request the relay forwards upstream from inside that span carries
  the same trace on, so browser → relay-2 → relay-1 is one trace. The relay
  declares the `TRACE_CONTEXT` Setup Option in its SERVER_SETUP, and an
  upstream relay's SERVER_SETUP gates the forwarded requests the same way. It carries `session_id` and `app_id`, and links to
  the `relay.session` span, so a session's trace holds only its lifecycle
  (`relay.session.disconnected` or `relay.session.protocol_violation` ends
  it) and a request's trace holds only that request's work. A span that
  outlives its request (`relay.dataplane.egress.track`,
  `relay.upstream.ingress`,
  `relay.inter_relay.session`) roots its own trace too, linked to the
  request: a tracing span closes only after its children, so a long-lived
  child would hold the request's trace open, unexported, for as long as it
  lives. The task behind such a span only borrows the request span to link
  it; keeping a `Span` handle would hold the request span open the same way. Before dispatching, it runs `auth::request_gate::authorize_request` against the session's
  `VerifiedToken` (looked up once when the worker starts and replaced when a
  TRACK_STATUS token refresh succeeds). PUBLISH, PUBLISH_NAMESPACE and
  PUBLISH_NAMESPACE_DONE need the `publish` claim; SUBSCRIBE, SUBSCRIBE_NAMESPACE,
  standalone FETCH and a TRACK_STATUS status query (no AUTHORIZATION TOKEN)
  need `subscribe`; a token-refresh TRACK_STATUS, joining FETCH and every other
  message pass through (they reference an already authorized request).
  PUBLISH_NAMESPACE_DONE is gated although it ends a PUBLISH_NAMESPACE because
  it carries a namespace rather than a Request ID, so nothing ties it to an
  authorized request. A denied request is answered with the message's
  `*_ERROR` carrying `UNAUTHORIZED (0x1)` and the sequence is never invoked; a
  denied PUBLISH_NAMESPACE_DONE, which has no error response, is logged and
  dropped.
- `SessionWorkers` (`session_workers.rs` spawns and reaps,
  `session_workers/session_worker.rs` is the per-session worker,
  `session_workers/session_cleanup.rs` the terminal cleanup): each worker
  reads only its own session, so a slow or blocked session can never
  head-of-line-block another. Workers process one event at a time, fully
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
  matching `Producer`, whose ingest stops with it. When that was
  the last upstream subscription of its track, the track's downstream
  registrations end with it, handed the received status code and reason (see
  "Egress"); otherwise the remaining publishers keep feeding them. No UNSUBSCRIBE is sent, since the publisher
  already ended the subscription.
- Events for control messages without relay-side logic yet
  (PUBLISH_NAMESPACE_CANCEL, FETCH_CANCEL) are logged in the
  event span and dropped by the worker; they have no `sequences` entry.
- Two relay-internal events are reported by the ingest path (not by a peer)
  to the upstream publisher session's mailbox:
  - `MalformedTrackDetected(track_key)`, raised by the reader that
    latched the track. Handled by
    `sequences::malformed_track::MalformedTrackCleanup`: remove every
    `Producer` of the track, since the cache latch makes the whole track
    malformed whichever publisher's object conflicted, and send each
    publisher UNSUBSCRIBE (§2.5 MUST); dropping the producers stops their
    ingests. Duplicate reports are idempotent (the track entry is only found
    once).
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
  client sessions. Leaving the repository first closes the race with a
  SUBSCRIBE_OK from this session that arrives concurrently in its
  `PendingProducerTask`. Either the directory removal finds the producer, or
  the check after adding it (`end_if_publisher_gone`) finds the session gone
  and ends the subscription itself; a creation then waits for the next
  answering publisher.

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
  to its sender; draft-18 §6.1 echoes NAMESPACE too).
- `InMemoryLocalPubSubDirectory` (`directory.rs`) — the relay's in-memory
  registry of publish/subscribe namespaces and tracks. Namespaces and
  prefixes are kept `/`-joined, and a prefix matches a namespace field by
  field (`domain::track_key::namespace_has_prefix`, draft-14 §9.28), the way
  `authorize` matches the granted path: `a/b` covers `a/b/c`, not `a/bc`, and
  the empty prefix covers every namespace. The
  join keeps field boundaries because `authorize` rejects a field containing
  `/`. Each namespace entry
  carries its sessions' `SessionPeer`: client-owned Redis routes are cleaned
  up when the last *client* leaves, while client namespace subscribers get
  PUBLISH_NAMESPACE_DONE only once no client or relay session publishes the
  namespace, and relay namespace subscribers once no client does (split
  horizon, see "SUBSCRIBE_NAMESPACE sequence"). Namespace subscriptions are keyed by
  session and `NamespaceInterest` (`NamespaceSubscription`: the interest, the
  peer and the Forward State of the PUBLISH messages it receives), and
  `get_namespace_subscribers(namespace, wanted)` returns those that ask for
  `wanted`: a draft-18 session's SUBSCRIBE_NAMESPACE and SUBSCRIBE_TRACKS for
  one prefix are two entries, and a draft-14 SUBSCRIBE_NAMESPACE is one
  `NamespacesAndTracks` entry. Each version's semantics come from the moqt
  handler, so a draft-14 and a draft-18 session are fanned out to by the same
  rules. `unregister_publish_namespace` returns a `PeerRemoval`
  (`NotRegistered`, `ClientsRemain`, `NoClientsRemain`): a
  PUBLISH_NAMESPACE_DONE from a client that did not publish the namespace is
  ignored, so it neither reaches the subscribers nor withdraws the Redis
  route. One from a relay session is applied without a registration, because
  a relay withdraws over its inter-relay session namespaces this relay never
  registered for it. A `Track` (`directory/track.rs`) holds one `Producer` per
  publisher session feeding it (draft-14 §8.2) and one `Consumer` per
  downstream subscription, keyed by (session, request id) and indexed in
  `consumer_tracks`; consumers refer to the track, not to a publisher. A
  `Producer` is its `ProducerState` (the `moqt::Subscription` handle, origin,
  peer, expires, Content Exists) plus the `IngestTask` reading the publisher;
  sequences read a `TrackState`, the producers without their ingests. A
  `Consumer` holds the senders its egress runner selects on: the stop
  `oneshot<PublishDoneReason>`, the `DeliveryControl` watch and the
  `DeliveryStats`. Dropping a producer stops its ingest and dropping a
  consumer stops its runner, so whoever removes an entry ends its task;
  the UNSUBSCRIBE a removal leaves due is sent by the caller, from the
  handle a release hands back (`ReleasedUpstreamSubscription`), because moqt
  addresses UNSUBSCRIBE / PUBLISH_DONE / FETCH_CANCEL by the subscription or
  fetch handle rather than by its request id. The track lives while any
  producer feeds it. The last consumer leaving releases its
  SUBSCRIBE-initiated producers, while PUBLISH-initiated ones stay until their
  publisher ends them. `remove_session` returns everything cleanup needs.
  A session's edges are found in the directory rather than indexed on the
  session: `tracks_fed_by` lists the tracks a session holds a producer of
  (session cleanup, GOAWAY from a relay peer, the drain release) and
  `consumer_tracks` its consumers.
- A `Track` also carries its **creation state** (`CreationState`: `Creating`
  until a producer feeds it, then `Created`; `Failed` with the SUBSCRIBE_ERROR
  to relay once every upstream SUBSCRIBE sent for it failed, after which the
  entry is gone), the number of upstream SUBSCRIBEs still pending for it,
  the **reservations** of the subscribers between finding it and registering
  their consumer (a `TrackReservation` guard; the track wants its producers
  for them already), and the sessions a forwarded PUBLISH is on its way to.
  `track_state` is `None` while no producer feeds the track, so a track being
  created is invisible to every sequence but SUBSCRIBE.

### SUBSCRIBE sequence (the central flow)
1. **Find-or-create upstream subscription.** `track_or_begin_creation`
   answers in one lock: `Ready` (producers feed the track), `Creating`
   (another SUBSCRIBE is creating it; wait on its `CreationState` watch) or
   `Started` (the entry was just inserted `Creating`; this SUBSCRIBE resolves
   the publishers and sends upstream SUBSCRIBE to every one of them at once,
   draft-14 §8.4). Each upstream SUBSCRIBE runs in its own
   `PendingProducerTask` (`sequences/subscribe/pending_producer_task.rs`):
   on SUBSCRIBE_OK it starts the ingest (`sequences::start_ingest`, which
   needs the publisher session's `Subscriber`, mailbox and span) and adds the
   `Producer`, which completes the creation; a publisher the track does not
   take any more — it ended, nobody watches it or is reserved on it, or the
   publisher already feeds it — is unsubscribed again, and a failed SUBSCRIBE
   counts the pending request down, the last failure of a creation failing it.
   A publisher that never answers — e.g. a session that died without closing
   and lingers until its idle timeout — therefore delays nobody, and the
   SUBSCRIBE fails only when every publisher refuses it. Concurrent
   subscribers to the same track produce one upstream subscription per
   publisher. Without any publisher to resolve, the creator abandons the
   creation, which fails it unless a PUBLISH created the track meanwhile.
2. **Publisher resolution** (`UpstreamPublisherResolver`): every local client
   publisher, newest first (session ids grow with time), and, for a SUBSCRIBE
   from a client, every remote relay the route registry lists as publishing the
   namespace or the track, once per relay (see "Cascading relays"). Resolution dials nothing: each upstream
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
   removed — which returns the consumer's `DownstreamRunnerSignals`. Start
   the egress runner with them (`sequences::start_egress`, which needs the
   subscriber session's `Publisher` and mailbox) and wait for the runner's
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
downstream subscriber and that the publisher does not feed yet, each in a
`PendingProducerTask`, which adds the publisher once it answers.
A PUBLISH_NAMESPACE from a relay does the same for the publishing relays the
route registry lists, reached over the sessions `InterRelayConnectionManager`
keeps per relay, and only for tracks a client watches (see "Cascading
relays"). A PUBLISH for a track the relay already receives adds its publisher
to the track the same way.

### PUBLISH sequence
1. For a client-origin PUBLISH, register the track-publisher route; a failure
   answers PUBLISH_ERROR INTERNAL_ERROR, like PUBLISH_NAMESPACE.
2. Start the ingest and add the publisher to the `Track` as a `Producer` of
   origin `Publish` (it survives without consumers), which replaces and
   unsubscribes a SUBSCRIBE-initiated producer of the same publisher, and
   register the PUBLISH for namespace lookups.
3. Forward PUBLISH to every local session whose namespace subscription for a
   matching prefix asks for tracks (draft-14 §8.4, draft-18 §9.5: a
   SUBSCRIBE_TRACKS, not a SUBSCRIBE_NAMESPACE), with that subscription's
   Forward State (draft-18 §10.19), and, for a client-origin PUBLISH, to every
   remote relay holding a namespace-subscriber route, dialled via
   `InterRelayConnectionManager` (Forward State 1: the remote relay applies
   its own subscribers'). A subscription that leaves out its session's own
   tracks (draft-18 §6.1) gets no PUBLISH for a track that session
   PUBLISHed; a draft-14 one is echoed it (draft-14 §6.1).
4. PUBLISH_OK to the publisher.

Each forward in step 3 is a `DownstreamPublish`
(`control_plane/sequences/downstream_publish.rs`), which treats the forwarded
PUBLISH as a downstream subscription of the track (§5.1: a PUBLISH_OK
establishes a subscription):
- a subscriber that already has a downstream subscription to the track, e.g.
  from the PUBLISH of an earlier publisher of it, or a PUBLISH for it still
  on its way (`begin_forwarded_publish`, released when the answer is in),
  gets no second PUBLISH;
- a PUBLISH moqt answers with `RequestFailure::PublishBlocked` (the
  subscriber's stream limit left no stream, so moqt sent PUBLISH_BLOCKED on
  its SUBSCRIBE_TRACKS) registers nothing, and the directory keeps the
  session and track until that track subscription ends, so no PUBLISH for
  the track goes to it again (draft-18 §6.1 MUST NOT);
- the PUBLISH advertises the Largest Location resolved like a fresh
  SUBSCRIBE (max of the cache and the track's Content Exists), and the egress
  runner starts from the same location;
- on PUBLISH_OK the subscription is registered under the forwarded PUBLISH's
  Request ID with the receiving session's `SessionPeer` — so UNSUBSCRIBE,
  SUBSCRIBE_UPDATE and the split-horizon counts treat it like a SUBSCRIBE —
  and `start_egress` starts its runner with the parameters of the
  PUBLISH_OK. The runner's acknowledgement signal is already resolved,
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
Also handles draft-18 SUBSCRIBE_TRACKS, which moqt raises as the same event
with `NamespaceInterest::Tracks`. Refuse with PREFIX_OVERLAP a subscription
whose prefix is a prefix of, equal to or an extension of one the session
already holds for an overlapping interest (draft-14 §9.28; draft-18 §10.18 /
§10.19 compare SUBSCRIBE_NAMESPACE and SUBSCRIBE_TRACKS only with their own
kind). Otherwise register the prefix under the session and interest (and,
for the first client subscriber of the prefix, its Redis route), reply
SUBSCRIBE_NAMESPACE_OK, and only then forward what already
matches and the subscription asks for: for namespaces, PUBLISH_NAMESPACE for
published namespaces (reply handled by a `PublishNamespaceResponseTask`); for tracks, a `DownstreamPublish`
with the subscription's Forward State for each published track (one per
publisher session), except those its own session PUBLISHed when it leaves
them out. PUBLISH_NAMESPACE and PUBLISH_NAMESPACE_DONE go to the subscriptions
that ask for namespaces only. The OK goes first because draft-14 §9.28 forwards
the existing messages once the subscription succeeded; a subscriber that
handles PUBLISH only after its SUBSCRIBE_NAMESPACE_OK would otherwise never
answer the PUBLISH before the request timeout.

A namespace published between the registration and the read of what already
matches reaches the subscriber both from the PUBLISH_NAMESPACE sequence and
from this one; so can a namespace announced by a second publisher or by both
a local publisher and a relay. `ControlMessageForwarder` therefore records in
`SessionRepository` each namespace it announces on a session and sends
PUBLISH_NAMESPACE only for one not yet announced there, and
PUBLISH_NAMESPACE_DONE to a client only for one that was. A relay session gets
PUBLISH_NAMESPACE_DONE either way: on draft-14 a relay that learned the
namespace from a route alone applies a DONE without a registration.
UNSUBSCRIBE_NAMESPACE forgets the
namespaces under its prefix, so a later SUBSCRIBE_NAMESPACE announces them
again.

**Namespaces other relays publish.** For a client's subscription that asks for
namespaces, the namespaces under the prefix that the route registry lists on
other relays and no local session publishes are not announced from the
registry. The relay sends SUBSCRIBE_NAMESPACE for each one, with the
namespace itself as the prefix and namespaces only, to every relay publishing
it, over the session it dialed (`InterRelayConnectionManager`), unless a
subscription of that session already covers it. The publishing relay answers
NAMESPACE (draft-18) or PUBLISH_NAMESPACE (draft-14), which this relay
handles like any relay's PUBLISH_NAMESPACE and so forwards to its client
subscribers, and withdraws it on the same request when its publisher leaves.
draft-18 withdraws a namespace only on the request that told of it: by
cancelling the PUBLISH_NAMESPACE (§6.2) or with NAMESPACE_DONE on a
SUBSCRIBE_NAMESPACE (§10.17). A namespace announced from the registry reached
no request between the relays, so the publishing relay could never have told
this one that it ended, and the clients kept a departed publisher. A
publisher that left between the registry lookup and the SUBSCRIBE_NAMESPACE
is answered with no NAMESPACE, so its namespace is not announced at all.
`InterRelayConnectionManager::namespace_subscriptions`
(`cascading/relay_namespace_subscriptions.rs`) holds these subscriptions by
dialed session and namespace. One ends with UNSUBSCRIBE_NAMESPACE when its
namespace is withdrawn on it, and when the last client subscription covering
the namespace leaves (`cleanup_empty_namespace_subscription`); all of a
session's end with the session.

A relay's SUBSCRIBE_NAMESPACE is served from this relay's clients only (split
horizon): it is told of the namespaces a client publishes here, never of
those another relay announced, and is not subscribed further. A namespace a
relay session announces reaches client subscribers only, and a relay
subscriber is told PUBLISH_NAMESPACE_DONE once no client publishes the
namespace, whatever relays still do.

A draft-18 update of the prefix (`SubscribeNamespaceUpdate`, §10.9.2) is
authorized for the new prefix and goes through the same sequence
(`handle_update`). It is refused with PREFIX_OVERLAP when the new prefix
overlaps another subscription of the session with the same interest, the
moving one not counted (§10.2.14). Otherwise the new prefix is registered as
a new subscription would be, the update answered, and the old prefix released
as an UNSUBSCRIBE_NAMESPACE would release it; then what the new prefix
matches and the old prefix did not is offered as above: what the old prefix
covered was offered when the subscription got it, so no track is PUBLISHed
and no namespace announced twice (an update has no effect on existing
subscriptions), whether or not the subscriber accepted the PUBLISH. An update to the prefix the
subscription already has is accepted and changes nothing. A refused update
ends the subscription, which moqt reports as UNSUBSCRIBE_NAMESPACE for the old
prefix. So does one moqt answered TIMEOUT before the relay accepted it: `ok`
reports `PrefixUpdateAnswer::Failed`, the new prefix is released again and
nothing is offered.

### SUBSCRIBE_UPDATE sequence
Subscriber Priority, Forward State, Start Location and End Group are applied.
Every downstream registration owns a `watch::Sender<DeliveryControl>`
(`domain/delivery_control.rs`: the Subscriber Priority, the Forward State, the Start Location resolved
from the filter and the Largest Location, the End Group, the track's Largest
Object at the latest update, and counts of the updates that moved the start
back and of those that turned Forward on) next to its runner stop sender. The End Group is kept as the first group
past the range, `None` when open-ended: an AbsoluteRange End Group is the last
group included (§9.7) and is stored plus 1. A SUBSCRIBE starts it at the
SUBSCRIBE's Forward State (§5.1 of draft-14 and draft-18: no object is sent at
Forward 0), while the relay's own upstream SUBSCRIBE always asks for Forward
1; a forwarded PUBLISH starts it at its PUBLISH_OK's Forward.
An update applies to the (session, Subscription Request ID) registration
through `session::handler::SubscribeUpdateHandler`: Subscriber Priority and
Forward are taken as sent (moqt gives a draft-18 REQUEST_UPDATE without
SUBSCRIBER_PRIORITY the subscription's current value), and the filter `filter()` returns is resolved against the track's Largest
Object like a SUBSCRIBE's (a draft-14 SUBSCRIBE_UPDATE states its range as an
absolute filter; a draft-18 REQUEST_UPDATE without SUBSCRIPTION_FILTER keeps
the range). An update that moves the Start Location back or the End Group out
is applied only when `SubscribeUpdateHandler::may_widen` allows it;
otherwise the session is closed with PROTOCOL_VIOLATION (draft-14 §9.10: the
publisher MUST). draft-18 §10.9.1 allows both: a start moved back is served
like a new SUBSCRIBE with that start, from just past the Largest Object at the
update (see "Egress"), and the subscriber FETCHes what lies before it. An
update for no registered subscription is logged and dropped. The upstream
subscription keeps Forward 1 and its own range, so the cache keeps filling for
FETCH. Every applied update is answered through `SubscribeUpdateHandler::ok`
with the track's Largest Object, resolved like a SUBSCRIBE's (draft-18
§10.2.11; draft-14 sends no answer). When `ok` reports that the subscriber was
given that location, an update that turns Forward from 0 to 1 also makes it
the registration's joining location (draft-18 §5.1), so a later Joining FETCH
ends there; draft-14 keeps the SUBSCRIBE_OK's Largest Location (§9.16.2).
When `ok` reports `UpdateAnswer::Failed` instead, moqt has already answered
the update REQUEST_ERROR TIMEOUT because the session worker reached it after
the control message timeout. The update is not applied, and the subscription
ends as draft-18 §10.9.1 requires (`UpdateVerdict::EndSubscription`; an
update the draft forbids returns `UpdateVerdict::CloseSession`, which the
session worker closes the session for): the UNSUBSCRIBE sequence removes the
registration, handing its runner PUBLISH_DONE UPDATE_FAILED, which it sends
once its streams are done, and releases the upstream subscriptions left
unneeded. The relay itself never branches on the draft.

### NEW_GROUP_REQUEST sequence
A subscriber asks the Original Publisher for a new group with
NEW_GROUP_REQUEST (draft-18 §10.2.13), in a SUBSCRIBE or a REQUEST_UPDATE.
`sequences/new_group_request.rs` passes it upstream:

- The SUBSCRIBE that creates the track: every upstream SUBSCRIBE carries the
  parameter (`PendingProducerTask`), and the track's `NewGroupRequestGate`
  records it as sent.
- A SUBSCRIBE to an established track, after its SUBSCRIBE_OK, and a
  REQUEST_UPDATE, after the SUBSCRIBE_UPDATE sequence applied it: the relay
  sends a REQUEST_UPDATE with the same value on the upstream subscription of
  every publisher whose track properties carry DYNAMIC_GROUPS (§12.6), both
  those it subscribed to and those that sent PUBLISH.

`domain/new_group_request_gate.rs` decides whether a request goes upstream.
It is held in the track (`directory::Track`), so it counts the requests of
all subscribers together. A request is not sent when:

1. no publisher of the track has dynamic groups (§10.2.13);
2. its value is not 0 and not above the track's Largest Group, resolved like
   a SUBSCRIBE's: the group it asks for already began (§10.2.13);
3. the last request sent is still outstanding and is not smaller. It is
   outstanding until the Largest Group increases (§10.2.13);
4. the last request was sent less than 2 seconds ago
   (`NEW_GROUP_REQUEST_INTERVAL`).

Rule 4 is not in the draft, whose §10.2.13 has the relay send every request
that passes rules 1 to 3. It bounds the keyframes a publisher encodes when
many subscribers join, and the group the earlier request started is then at
most 2 seconds long, so the subscriber held back joins it by Joining FETCH
(`moqt::Subscriber::subscribe_group_start` does so on its own). A relay
subscribing to another relay sends and gates the request the same way, so
each relay on the path applies the interval.

The request is not answered to the subscriber beyond the REQUEST_OK of its
REQUEST_UPDATE; a subscriber cannot tell whether it was passed on.

### FETCH sequence
Resolve the track and object range (Standalone from the message; Joining
from the downstream subscription's registration, which records its track,
filter and joining location), reply FETCH_OK,
then start a `FetchDeliveryTask` (`sequences::start_fetch_delivery`), which
serves the range from `TrackCache` over a new uni stream (see "Fetch
delivery" under "Egress").
A Joining FETCH is checked in this order: an unknown joined Request ID gets
INVALID_JOINING_REQUEST_ID; then `FetchHandler::check_joining` applies the
session draft's rule to the registration's filter and current Forward State
(draft-14 §9.16.2: a filter other than Largest Object closes the session with
a PROTOCOL_VIOLATION; draft-18 §10.12.2: Forward State 0 gets INVALID_RANGE);
then a subscription accepted before the track had objects gets INVALID_RANGE;
then the range. The session worker handles a session's events in order, so
every update received before the FETCH is applied first, as draft-18
§10.12.2 requires.

A start after the track's Largest Object gets INVALID_RANGE (draft-14
§9.16.3, draft-18 §10.12.3) only while live ingest runs, the Largest resolved
like a SUBSCRIBE's: the greater of the cache's largest location and the
upstream SUBSCRIBE_OK's. A start up to the upstream Largest that the cache
has not ingested yet is not refused; it goes upstream.

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
fail; a departed session gets no delivery.

The fill is store-and-forward: egress starts once the upstream response
ended, from the cache. A draft-18 FETCH's FILL_TIMEOUT (§10.2.5) bounds the
wait, counted from the FETCH's arrival and cut to
`MOQT_FETCH_FILL_TIMEOUT_SECS` (`FillBudget`). When it runs out, the
upstream FETCH is cancelled and the range is served with what arrived, the
rest reported as unknown. FILL_TIMEOUT 0 forbids waiting: a range the cache
does not cover is served at once from the cache with the rest unknown,
going upstream only when the cache holds no object at or after the start,
so FETCH_OK has an End Location, and then waiting for no object. Without
FILL_TIMEOUT (always on draft-14) the relay waits up to
`MOQT_FETCH_FILL_TIMEOUT_SECS`, also counted from the FETCH's arrival, and
then does the same (§10.2.5 "an
implementation specific duration"); toward a draft-14 downstream moqt
cannot mark the rest unknown and resets the FETCH there, with
INTERNAL_ERROR on the wire. A response whose upstream marked objects
unknown is served with the known objects and the unknown ranges reported
(see Fetch delivery), on either draft.

### TRACK_STATUS sequence
A status query is answered from the relay's own state only. For a track with
a `Track`, TRACK_STATUS_OK carries the newest producer's
Expires and the Largest Location resolved as on the SUBSCRIBE fast path (the
cache's largest location, else the largest Content Exists of its upstream
subscriptions). A latched malformed track is answered MALFORMED_TRACK. Any
other track is answered TRACK_STATUS_ERROR `NOT_SUPPORTED (0x3)`: draft-14 §9.20 lets a
relay without an active subscription forward the request or subscribe
upstream (MAY), and this relay does neither.

### GOAWAY sequence
`sequences::go_away::GoAway`, draft-14 §3.5. A GOAWAY from a client is only
logged: the client is leaving and its state goes with its session. A GOAWAY
from a relay-peer session means that relay is draining and will close the
session, so for every track that session feeds the sequence resolves the
publishers again through `UpstreamPublisherResolver` (local client publishers
and the relays still `active` in the route registry; the draining relay is no
longer active there) and subscribes those not already feeding the track, each
in a `PendingProducerTask`, like a newly announced publisher. The departing session's own subscriptions end through the ordinary
cleanup when it closes; a track another publisher joined in the meantime keeps
its downstream subscribers.

## Authentication and authorization (`modules/auth`)

Building blocks; the session intake wiring is described above, the
per-request authorization gate under "Event pipeline".

- `verified_token.rs` — `VerifiedToken`, the claims returned by the Verify
  Token Service (VTS): `app_id`, optional `publish` / `subscribe` namespace
  paths (already split on `/`; the empty claim `""` is the app root, i.e. an
  empty path), `is_relay`, `expires_at`, and `relay_id`, the `relayId`
  claim of a relay token (never read from a client token).
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
  transport error → `Unavailable`. 3 s timeout. The call runs in a
  `relay.vts.verify` span whose context goes out as a W3C `traceparent`
  header and its copy `x-original-traceparent`, so the VTS's spans join the
  relay's trace.
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
`IngestTask` (`ingest_task.rs`) is the ingest of one publisher's track,
owned by the track's `Producer`:

- `sequences::start_ingest` spawns it with the publisher session's
  `Subscriber`, the track cache, the publisher's mailbox sender and the
  `CacheStamp`. The task creates the data receiver (cancelled by the stop
  `watch` when the producer is dropped meanwhile), then reads it with
  `read_track` (`track_reader.rs`). The receiver
  (`session::data_receiver::receiver::DataReceiver`) yields the
  subscription's subgroup streams and datagrams in arrival order
  (`UpstreamData`), so a draft-18 track that mixes them (§11) is ingested
  as it comes: `read_track` spawns a `read_stream` (`stream_reader.rs`) per
  subgroup stream and inserts each datagram through a `DatagramGroup`
  (`datagram_reader.rs`), which keeps the datagram group being filled open in
  the cache. The track counts as live (`begin_live_ingest`) from its first
  data on. Every reader of one publisher's track shares one `TrackIngest`
  (track key, cache, publisher mailbox sender, stop receiver, stamp).
  **Every publisher is ingested** (draft-14 §8.2): all publishers of a track
  insert into the same `TrackCache`, whose §8.1 duplicate handling
  deduplicates them. A producer registered for a publisher that already
  feeds the track replaces it, and the replaced producer's ingest stops with
  it; dropping a producer tears down only its own readers. Stream readers
  outlive their accept loop until their stop sender is signalled or dropped.
- Readers convert every object moqt hands them (`moqt::object::Object`, ids
  already resolved) into a canonical `CachedObject` and insert it into
  `TrackCache`; an object whose payload arrives in pieces is published as an
  arriving object first (see "Arriving objects" under Cache). A subgroup header (`SubgroupStart`) is not cached: the reader
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
  no per-subgroup delta state. An object of a response that carries no
  forwarding preference (draft-14, `carries_forwarding_preference()`) is
  marked `forwarding_inferred`.
- The cache's sticky §2.5 malformed latch is set by an ingest reader: by the
  insert that conflicts, or when moqt reports a malformed track — a draft-14
  upstream subscription that switched between subgroup streams and datagrams,
  or an object that conflicts with a copy the same upstream session delivered
  before (`MalformedTrackError` from `read_track` or `FetchIngest`,
  `StreamReceiveError::MalformedTrack` from `read_stream`). That reader is always
  present to report `MalformedTrackDetected` into the event pipeline — no
  standing watcher is needed. Two publishers that use different forwarding
  preferences for one track are not detected unless their objects conflict. Downstream, `EgressRunner` watches the same
  latch and terminates subscriptions with PUBLISH_DONE(MALFORMED_TRACK);
  `FetchIngest` bails on the latch and sends upstream FETCH_CANCEL for its
  own fetch.
- An upstream SUBSCRIBE or FETCH refused with MALFORMED_TRACK latches the
  track as well (`malformed_track::refused_as_malformed`): moqt refuses a
  draft-18 SUBSCRIBE_OK or FETCH_OK whose Immutable Properties are malformed
  (§12.7) that way, and so does a relay peer that detected the track
  malformed. No other publisher is tried, the downstream request is answered
  MALFORMED_TRACK, and the track's other upstream subscriptions end through
  `MalformedTrackCleanup` (called by the refused `PendingProducerTask`, which
  also fails the track's creation with MALFORMED_TRACK; raised as
  `MalformedTrackDetected` to the publisher's session by
  `UpstreamFetchTask`). A publisher answering SUBSCRIBE_OK later finds no
  track to feed and is unsubscribed. A
  PUBLISH with malformed Immutable Properties is refused by moqt itself and
  never reaches the relay, so it latches nothing. A SUBSCRIBE, FETCH or
  TRACK_STATUS for a latched track is answered MALFORMED_TRACK before any
  upstream request is sent; draft-14 defines that code for FETCH_ERROR
  only, so a draft-14 SUBSCRIBE_ERROR or TRACK_STATUS_ERROR carries
  INTERNAL_ERROR.
- The latch lives in the track's `TrackCache`, and eviction keeps a latched
  cache for the relay cache TTL after the latch was set even when it holds
  no object (a latch from an object withheld by moqt or from a peer's
  MALFORMED_TRACK caches nothing), so a request after the publisher left
  is still answered MALFORMED_TRACK. Past that, the track goes like any
  drained one and a later request finds no track.

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
  draft-18 §9.1 compares no status: the ingest asks moqt whether its upstream
  session compares it (`compares_duplicate_status`) and stamps the answer on
  each object as `status_may_change`, and the status counts only when neither
  copy carries it. A datagram's `ends_group` is not compared. Only a `forwarding_inferred`
  copy (from a draft-14 FETCH, §10.4.4) in the subgroup of its Object ID is
  the same object as a datagram; a draft-18 track may send an object in that
  subgroup and a datagram FETCH copy says so (§11.4.4), so elsewhere the
  preference is compared exactly. When a copy that carries its preference
  (live, or from a draft-18 FETCH) meets such an inferred copy that arrived
  first, the cache takes the forwarding preference from it, and from a
  datagram its `ends_group` too, so only an object no such copy has reached
  is left to the guess below. A FETCH served from the cache asks
  `TrackCache::fetch_forwarding` for each object's preference. To a session
  the track went to in one preference (see "Relaying across versions") the
  object goes as live delivery gave it. Otherwise an inferred copy in the
  subgroup of its Object ID, which no live copy has reached, goes as a
  datagram while the latest object live
  ingest inserted is a datagram (a draft-14 track keeps one preference), so
  a draft-18 FETCH marks it and it matches the live datagram the subscriber
  may hold; every other object goes as cached.
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
- Arriving objects (`track_cache/partial_object.rs`): a subgroup stream reader
  gets each object whose payload had not arrived with it as an object start
  and payload pieces (`moqt::object::SubgroupPartReceiver`). The reader calls
  `OpenSubgroupGuard::begin_object`, which publishes a `PartialObject` in the
  ledger's `partial_objects` unless `objects` holds that location or another
  stream already publishes it, appends each piece, and on the last one inserts
  the assembled object like any other (§8.1 checks included) before
  `finish`ing the writer. A writer dropped earlier (reset, stop, decode error,
  or 10 s (`STALLED_PAYLOAD_TIMEOUT`) without a payload piece, which also stops
  the stream) marks the object aborted, while a stream idle between objects has
  no deadline. Either way the object leaves `partial_objects`, so FETCH,
  eviction, the disk cache and the largest location only ever see complete
  objects. `next_subgroup_object_or_wait` returns `NextObject::Partial` when
  the next object of the subgroup is arriving and no cached object precedes
  it. A partial object wakes the track's `Notify` only when it is published
  and withdrawn; its pieces go through its own `watch` channel, so only the
  egress streams forwarding it wake per piece.
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
  becomes unknown") and a skipped id stays undecided. A fetch fill asks for
  ascending order, so every position up to the last object or marker it
  received is known except the ranges a `Fetch::UnknownRange` marked; it
  registers those known ranges when it ends (guarded by the eviction
  generation counter): through the end of the range at `Fetch::End`, through
  the last item received when the fill budget ran out. A range with
  an unknown part stays uncovered, so a later FETCH of it goes upstream
  again. Datagram objects register nothing.
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
- `resolve_fetch_range` serves a range reaching past knowledge only while
  live ingest runs, object 0 of every group in it is known, and each group
  whose knowledge stops inside the range still has an open subgroup to bring
  the rest. Otherwise it answers `NotCovered`, so the FETCH goes upstream
  before any FETCH_OK: a group whose upstream stream ended keeps its gap (a
  joining FETCH can reach into it when the publisher's SUBSCRIBE_OK reports
  objects sent after the relay stopped reading), and a stream reset after
  FETCH_OK can reach the subscriber before the stream's header.
- Eviction job (`eviction_job.rs`): every `RELAY_CACHE_EVICT_INTERVAL_SECS`
  (5 s) drop objects older than `RELAY_CACHE_TTL_SECS` (60 s) and release
  knowledge exactly for the removed locations. While live ingest runs, the
  object at the track's largest location is kept past the TTL: a late
  subscriber is answered Content Exists from the cache (§9.8) and fetches that
  object, so a sparse track such as a catalog stays joinable; a `TrackCache` entry is removed
  from the store only when it is empty and `Arc::strong_count == 1`, i.e. no
  ingress/egress holds it — avoiding races with new joiners — and not
  latched malformed within the TTL.
  A pass never walks the whole track: the disk cache keeps ~10^5 objects per
  track-hour, and the pass holds the write lock. The one exception is the
  end-of-broadcast purge, which walks every object once.
  `EvictionQueues` (`track_cache/eviction_queues.rs`) keeps three indexes:
  - objects by `received_at`, until their memory TTL is applied;
  - objects by `CachedObject::expiry`;
  - stored objects per disk segment.

  A pass visits only the objects these indexes mark as past the memory TTL,
  expired, or on a deleted segment. `resolve_fetch_range` uses the same
  indexes to ask whether a range holds an expired object that eviction has
  not removed yet. A group's aborted / finished marks and reopened runs are
  dropped once the group has neither objects nor open subgroups; a pass
  checks only the groups it removed objects from and the groups whose last
  subgroup closed since the last pass.
- MAX_CACHE_DURATION (draft-14 §9.2.1.3): ingress stamps every object with
  `expires_at` = this relay's receipt time + the value its upstream
  SUBSCRIBE_OK, PUBLISH or FETCH_OK carried. From that instant
  the object is never handed out: a live read skips it, a FETCH served from
  cache ends with `FetchInterrupted::Incomplete` (the stream is reset), and
  `resolve_fetch_range` answers `NotCovered` for a range that holds one, so the
  FETCH goes upstream. Eviction removes it at `expires_at` whether that is
  before or after the relay TTL, and the newest-object exception does not
  extend it. Objects received without the value keep the relay TTL.
  Downstream, the relay sends the value it received unchanged, as part of the
  track properties below. Each relay counts
  from its own receipt, so in a cascade an object can stay cached up to N ms
  per hop. A value above `RETENTION_LIMIT` (1 h) is cut to it; caching is
  optional (§8.1), so keeping an object for less is allowed.
- Track properties (`moqt::TrackProperties`: MAX_CACHE_DURATION and the
  draft-18 Track Properties moqt holds as received, Immutable Properties
  included) go downstream whole and unchanged (draft-18 §2.5). The relay
  never merges two sets; a newer one replaces the older.
  - SUBSCRIBE_OK and the forwarded PUBLISH carry
    `TrackState::track_properties`, the newest publisher's.
  - A relayed upstream FETCH_OK carries that FETCH_OK's set.
  - A FETCH_OK served from cache carries the same newest publisher's set
    while one is live. Once the last publisher has left, it carries
    `TrackCache::track_properties`, the set the relay served last, so it
    equals the last SUBSCRIBE_OK (§2.5 caches properties with the track).
    The cache follows the routing: each ingest start and stop records the
    publisher's set, and the served set is the newest live publisher's, so
    when the newest leaves first the older publisher's set takes over. An
    upstream FETCH_OK's set is served only while no publisher is live.
  - A draft-14 session sends none of the draft-18 properties (see
    "Relaying across versions" for Mandatory Track Properties).
- Disk cache (`disk_cache.rs`, on when `RELAY_DISK_CACHE_DIR` is set): the
  relay a client publisher sends to keeps the payloads for up to
  `RETENTION_LIMIT`; a downstream relay keeps its memory cache only and
  forwards a FETCH that misses it upstream. `start_ingest` and
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
`EgressTask` (`egress_task.rs`) runs the `EgressRunner` of one downstream
subscription; `sequences::start_egress` spawns it right after the consumer
is registered, with the subscriber session's `Publisher` and mailbox sender.
There is no stop command: the `Consumer` owns the
`oneshot::Sender<PublishDoneReason>` whose receiver `EgressRunner::run` selects on (biased,
before its delivery), so the runner lives exactly as long as the registration.
Whoever removes the consumer — UNSUBSCRIBE, the subscriber's or the publisher's session
cleanup, an upstream PUBLISH_DONE — stops the runner, and a consumer removed before its
runner first polled its signals yields a runner that never delivers; the order in which
different session workers touch the registration cannot leak a runner. Removal
drops the sender when the subscriber ended the subscription (draft-14 §5.1: the publisher
may destroy its state). Removal because the track's last upstream ended — its publisher's session went
away (TRACK_ENDED) or it sent PUBLISH_DONE (its status code) — sends a `PublishDoneReason`
instead, and the runner answers it with PUBLISH_DONE carrying that status and the number
of streams it opened, after its senders stopped (§9.12: no state is destroyed without
PUBLISH_DONE). A malformed track ends the runner the same way with MALFORMED_TRACK. A
subscription with an End Group ends the runner once its range is delivered (see
`EgressScheduler`): it sends PUBLISH_DONE SUBSCRIPTION_ENDED after every stream it opened
was closed or reset (§9.12), and the task then sends
`SessionEvent::DownstreamSubscriptionEnded` to the subscriber session's mailbox, whose worker removes
the registration through the UNSUBSCRIBE sequence, releasing the upstream subscriptions it
leaves unneeded. The
runner sends PUBLISH_DONE only once the subscribe sequence signals that SUBSCRIBE_OK went
out (`subscribe_ok_receiver`), so it never precedes SUBSCRIBE_OK; a runner stopped before
readiness drops it, the SUBSCRIBE fails with SUBSCRIBE_ERROR and no PUBLISH_DONE is sent. A
forwarded PUBLISH passes an already resolved signal: its PUBLISH_OK has been received.
`EgressRunner` splits into:

- `EgressScheduler` — listens on the cache's subgroup-opened channel and the cache,
  takes the delivery start from the registration's `DeliveryControl` (the
  Start Location of the draft-14 filter type, `NextGroupStart`,
  `LargestObject`, `AbsoluteStart` or `AbsoluteRange`; an absolute start at or
  below Largest is clamped to Largest+1), and emits one `GroupSendTask` per
  `SubgroupKey` and run generation: a reopened run is scheduled again from
  the later of the key's previous task start and the run's first object id,
  while further opens of an already scheduled run are skipped. It subscribes to open events first and then schedules every
  cached group at or after the start (newest first under Descending Group
  Order, §7.1), so a group that ingress opened and closed before the
  scheduler existed is still delivered. The scan skips a closed subgroup that
  holds nothing at or after the start, such as one only a joining FETCH
  filled. A task for it would end at once, and the open event of its live
  stream would then be dropped as already scheduled. When the channel
  reports lag it scans
  the cache the same way from just past the last open event it received, so a
  group whose open event was dropped is still delivered under either Group
  Order. While the
  registration's Forward State is 0 it drops open events, and a runner that
  starts at Forward 0 skips that cache scan. No object is sent at Forward 0
  (draft-14 §5.1, draft-18 §5.1): the send tasks hold their subgroups and reset
  their open streams (see `GroupSender`). An update that turns Forward to 1
  restarts the schedule like a new subscription, from just past the Largest
  Object at that update, so a subgroup opened while paused and still open
  delivers the objects after it on a new stream (draft-18 §11.4.3 allows a
  new stream after Forward 0→1), and the subscriber FETCHes the objects up to
  it. A count of these updates tells the scheduler and the send tasks about
  a pause they did not see, since a watch keeps only the latest state. The start is a
  lower bound in both paths: group ids may begin anywhere and skip values
  (§2.3.1), so the first delivered group is the first one at or above the
  start, not the start group itself. Groups at or past the End Group are never
  scheduled, and an open event reads the current `DeliveryControl`, so a
  SUBSCRIBE_UPDATE's later Start Location, smaller End Group or larger End
  Group applies to every subgroup opened after it. An update that moved the
  Start Location back (draft-18 §10.9.1; the restart count tells, even when
  the scheduler only sees the latest state) restarts the schedule like a new
  subscription: it scans the cache from the new start clamped past the
  Largest Object at the update and schedules the subgroups not scheduled yet.
  A subgroup scheduled before continues in its own task (see
  `GroupSender`). The range has ended, and the scheduler returns
  `SchedulerExit::EndGroupReached`, once the start lies at or past the End
  Group or the cache has opened a group at or past it: live groups arrive in
  ascending order, so no further group can enter the range (a subgroup of an
  in-range group that opens after a later group is not delivered). It checks
  this at start-up, on every open event (also at Forward 0) and on every
  `DeliveryControl` change. Dropping its task sender then lets `GroupSender`
  finish once its in-range streams have been delivered to their end.
- `GroupSender` — one task per subgroup: waits for the first object to send,
  only then opens the downstream uni stream (a subgroup that closes empty
  opens nothing), regenerates the SUBGROUP_HEADER from that object's canonical
  properties (explicit subgroup id, priority; extensions always declared
  present so no object can lose its extension headers), and streams objects
  until `next_subgroup_object_or_wait` reports the subgroup finished (FIN
  downstream) or aborted (RESET_STREAM downstream, INTERNAL_ERROR). An object
  still arriving is forwarded as it arrives: its start, then each piece the
  upstream delivered (cut-through), so the next hop does not wait for the
  last byte to reach the relay. When the upstream stream ends inside that
  payload, the downstream stream is reset with INTERNAL_ERROR, even if another
  publisher's stream later completes the object. Each task
  also watches the registration's `DeliveryControl`: when a SUBSCRIBE_UPDATE
  moves the range past its group (End Group at or below it, Start Location
  beyond it), the open stream is reset with CANCELLED (draft-14 §10.4.3). Past
  the End Group the task ends. Before the Start Location, and at Forward 0 or
  after a resume it did not see, it is held: it sends nothing and ends once
  its subgroup ends or the scheduler has returned (no update can restart
  delivery any more, and the runner sends PUBLISH_DONE only after every task
  ended), unless a later update moves the start back over the group
  or turns Forward to 1, after which the subgroup continues on a new stream
  from just past the Largest Object at that update. Every object the
  task sent lies at or before that location, so none is sent twice. Groups
  still in the range are delivered to their end. Datagram groups are
  re-emitted with the downstream track alias, and stop at the same range
  change. A downstream whose session mixes forwarding preferences
  (`Publisher::mixes_forwarding_preferences`, draft-18) gets every
  `SubgroupKey` in the preference its objects arrived with, so a draft-18
  track mixing subgroup streams and datagrams goes downstream mixed. For any
  other downstream (draft-14 §10) the first task the session's first
  subscription of the track schedules fixes the preference, and later keys
  of the other kind are converted (see "Relaying across versions"). One
  subscription runs at most `MAX_CONCURRENT_SEND_TASKS` (256)
  stream and datagram tasks. At the cap `GroupSender` stops taking tasks, so
  the scheduler blocks on sending the next one; once it reads open events
  again, its lag scan schedules the missed groups that are still cached.
  Each stream is opened with a `moqt::object::StreamPriority` (draft-14 §7.2):
  the subscriber priority `DeliveryControl` holds then, the first object's publisher
  priority, the subscription's group order with a per-subscription
  `GroupSequence` (incremented for every newer group id the sender sees,
  shared by the subgroups of one group), and the subgroup id.
  `StreamSenderFactory::next(priority)` applies it before the header
  is written. An update that changes the subscriber priority also reaches a
  stream that is open (§7.1: a best effort SHOULD be made for the objects not
  yet scheduled): its task sets the new priority with `DataSender::set_priority`
  before it reads the next object, so a task waiting inside a write applies it
  once that write returns. `next` waits for the subscriber's stream credit, so the task
  keeps only the publisher priority across it and reads the first object from
  the cache again afterwards: a subscriber that withholds credit cannot keep an
  object alive past its cache expiry. A task that has not opened its stream
  10 s (`STREAM_CREDIT_TIMEOUT`) after it starts waiting for the shared factory
  gives up on its group, and the subscription goes on. The sequence, not the group id, feeds the rank because group
  ids may be wall-clock timestamps; assigning it at open time means a still
  buffered tail of group N keeps outranking group N+1's keyframe without
  re-prioritizing streams already queued in the transport.
- Fetch delivery (`fetch_delivery.rs`) — `FetchDeliveryTask`, one task per
  FETCH served from the cache: it opens the fetch stream, drives the `FetchCursor` and writes each
  object as soon as the cursor yields it, so objects of already-closed groups
  reach the subscriber while a later group in the range is still open. The
  stream is FIN'd when the cursor is exhausted and reset with
  MALFORMED_TRACK / INTERNAL_ERROR when the cursor reports `Malformed` /
  `Incomplete`. A request with `FetchWait::Never` waits for nothing instead:
  `TrackCache::plan_fetch` (`track_cache/fetch_plan.rs`) lists the cached
  objects in delivery order with every stretch the cache does not know
  between them as `PlannedFetch::Unknown { last }`, an expired object
  counting as unknown, and delivery passes each stretch to
  `report_unknown_range`. moqt marks it with draft-18's End of Unknown Range
  and the delivery goes on, or resets the stream with UNKNOWN_OBJECT_STATUS
  where the draft cannot express it (draft-14, or a range ending in the
  group of the previous object), which ends the delivery. That second case
  is the usual one when a fill budget runs out: the stop lies inside the
  open group or the group a stalled fill was reading, so a draft-18
  downstream is reset there too, as §10.12 allows. This is a gap in
  draft-18's End of Unknown Range (§11.4.4.2), and an interop item: a plain
  RESET_STREAM lets the receiver discard the objects it has not read yet
  (RFC 9000 §3.2).
  `FetchWait::Until(deadline)` drives the cursor until the deadline and
  then serves the rest of the plan, the steps after the last object sent.
  A FETCH served from the cache takes its wait from `FillBudget`: `Never`
  for FILL_TIMEOUT 0, `Until` the end of a non-zero budget (draft-18
  §10.2.5). Without FILL_TIMEOUT it is `Until` the end of
  `MOQT_FETCH_FILL_TIMEOUT_SECS` when the downstream response can mark the
  rest unknown (`FetchHandler::marks_unknown_ranges`, draft-18), and
  `Unlimited` otherwise (draft-14), which waits for an open group as long
  as live ingest keeps it open.

### Relaying across versions

With `RELAY_MOQT_VERSIONS=draft-14,draft-18` draft-14 and draft-18 peers
share tracks. The relay never reads a session's draft: moqt holds the
per-draft rules and answers neutral questions, and every MUST of both drafts
holds on each hop.
- Track properties: a draft-14 session sends none of the draft-18 ones.
  moqt understands no Mandatory Track Property (draft-18 §2.5.1), so it
  refuses a PUBLISH, SUBSCRIBE_OK or FETCH_OK carrying one at receipt, and
  the relay never holds such a track. The relay passes the upstream
  UNSUPPORTED_EXTENSION to the downstream subscribers, as §2.5.1 requires;
  draft-14 has no such code, so a draft-14 subscriber gets NOT_SUPPORTED.
  Were a cached track to carry one, the subscribe sequence would refuse a
  draft-14 subscriber before registering anything
  (`SubscribeHandler::understands`), the FETCH sequence would refuse a FETCH
  it serves from the cache before FETCH_OK (`FetchHandler::understands`),
  both logged at WARN as the error response they send, and moqt's draft-14
  `publish` would refuse it toward a draft-14 peer.
- Forwarding preference: `GroupSender` keeps a track to a session that does
  not mix preferences in one `SinglePreference`, that of the first subgroup
  the session's first subscription of the track scheduled.
  `TrackCache::fix_single_preference` keeps it per session until the
  session ends (`TrackCacheStore::forget_downstream_session` in the session
  cleanup), so later subscriptions and every FETCH from the cache to that
  session give each object the preference and Subgroup ID it went live in
  (`SinglePreference::forwarding`): draft-14 §8.1 makes a copy that differs
  in either a conflicting duplicate, which moqt's `ObjectLedger` turns into
  a malformed track.
  - Started on a stream: each datagram object goes alone on a subgroup
    stream in a range reserved for converted datagrams: Object ID `n` goes
    on Subgroup ID 2^61 + `n` (`domain::single_preference`), and an Object
    ID at or past 2^61 is dropped. A real subgroup of the group never shares
    it, as it would were the Subgroup ID the Object ID: a second stream of
    one subgroup with another final object makes the track malformed
    (draft-14 §2.5). Publishers number subgroups from 0, so none is expected
    in the range. The datagram's End of Group becomes the header's
    `ends_group`, and the stream ranks last in its group.
  - Started with a datagram: stream objects go as datagrams, an End of
    Group status object included, and one larger than the session's maximum
    datagram size is dropped as draft-14 §10.3 drops any datagram: moqt
    logs it at DEBUG and reports it (`DatagramDelivery::TooLarge`), and the
    subscription's `datagrams_dropped` stat counts it. A FETCH
    gives them as datagrams too, which a draft-14 FETCH sends in the
    subgroup of their Object ID (§10.4.4), as the live datagram's duplicate
    check expects.
- FETCH: a draft-14 FETCH response carries no preference, and an inferred
  datagram copy is served to a draft-18 FETCH as described in the cache
  section.
- Cascading: the inner endpoint accepts the drafts of `RELAY_MOQT_VERSIONS`
  and the dialing relay offers the same, so between two relays that enable
  draft-18 a cascaded draft-18 track keeps its Track Properties and its
  mixed forwarding preference. A hop to a relay on draft-14 only goes in
  draft-14, which carries neither.

## Cascading relays (`modules/cascading`)

- `RelayRouteRegistry` trait: `NoopRelayRouteRegistry` (single-relay, no
  `REDIS_URL`) or `RedisRelayRouteRegistry` (relay info hash and
  namespace-publisher / track-publisher / namespace-subscriber routes with a
  15 s TTL). `find_namespace_publishers_by_prefix` lists the
  namespace-publisher keys with a `KEYS` pattern built from the escaped
  prefix, which matches characters, and keeps only the keys under the prefix
  field by field (`namespace_has_prefix`). The
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
  subscriber relay. A namespace published before a relay registered its
  subscriber route is found there by prefix; that relay subscribes it at the
  publishing relay with SUBSCRIBE_NAMESPACE, so the publishing relay can
  withdraw it (see "SUBSCRIBE_NAMESPACE sequence").
- **Track-publisher routes**: a track a client publishes with PUBLISH alone
  has no namespace route, so `Publish` registers
  `route:publisher:track:<namespace>:<name>` for it. The key lies outside the
  namespace-publisher prefix, so `SubscribeNamespace` never announces a
  namespace no publisher announced. The route is withdrawn when no client
  publishes the track with PUBLISH on this relay any more: at the publisher's
  PUBLISH_DONE or session close.
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
  over raw QUIC (`moqt::QUIC`), presenting this relay's own JWT
  (`AUTH_RELAY_TOKEN`) in CLIENT_SETUP, and registers the
  session as `SessionPeer::Relay` with `VerifiedToken::full_access()` naming
  the dialed relay in `relay_id`, reusing it afterwards. From then on the remote relay behaves like any upstream
  publisher session.
- The dialing relay verifies the remote relay's certificate against
  `InterRelayClientConfig::certificate_verification` before CLIENT_SETUP, so
  the relay token, which grants full access and is long-lived, never reaches
  a host that cannot prove it is the advertised relay. The certificate must
  name `RELAY_ADVERTISE_HOST`. With `RELAY_INNER_CA_FILE` set only the
  certificates of that file are trusted (compose: the self-signed certificate
  relay-a and relay-b share); otherwise the OS roots are (production: a
  publicly trusted certificate for the relay's host name). There is no client
  certificate: the dialed relay authenticates the dialing one by its token.
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

The relay sends its own statistics as OTLP log records (format:
`telemetry::relay_stats`, consumer: `docs/architecture/observability/architecture.md`),
the path its traces and logs already take, so the observability server needs
no list of relays and no session to each.

- `StatsExportTask` runs only with OTLP on. Once per second it collects a
  snapshot, emits it through `telemetry::EventLogger` (`EventName`
  `relay.stats.snapshot`, the JSON in its `snapshot` attribute) and records
  its `process` values through `telemetry::relay_stats::ProcessGauges`
  ("Relay gauges" in the observability architecture). It reads the
  same locks as the control plane, so a stuck runtime or lock stops the
  records and the relay leaves the live view.
- `StatsCollector` reads, without holding any lock across an await:
  `SessionRepository::session_states` (peer, `app_id`, `Session::transport_stats`
  and `transport_addresses`), `InterRelayConnectionManager::dialed_relay_ids`
  (the relay at the other end of each session this relay dialed; an accepted
  inter-relay session is matched by the consumer through the relays' known
  addresses, see `docs/architecture/observability/architecture.md`),
  `InMemoryLocalPubSubDirectory::active_upstream_tracks` and
  `downstream_subscription_states`, each track cache's `IngressStats`, and
  `TrackCacheStore::occupancy` (with the disk cache's stored bytes and byte
  cap), plus `SessionRepository::client_session_count`
  (the count a drain waits for) and the drain's start time, which
  `InMemoryLocalPubSubDirectory::begin_draining` records. RSS comes from
  `/proc/self/status` and is absent elsewhere. `active_upstream_tracks` lists every publisher feeding a
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
- `RemoteAddressWatchTask` (`session/remote_address_watch_task.rs`) runs for
  every session, with or without OTLP: quinn reports no event when the peer
  migrates its connection (RFC 9000 §9), so the task polls
  `Session::transport_addresses` once per second and logs `connection
  migrated` at INFO with `session_id`, `previous_remote_address` and
  `remote_address` when the address changes, NAT rebinding included. The
  `SessionEntry` owns it, so removing the session stops it.
- QUIC statistics are per connection and describe what the relay sent, so a
  client's uplink shows no loss or cwnd; the received flow-control and
  reset frame counts are its uplink signals.

## Key invariants

- **A worker reads only its own session**: nothing shared sits between a
  session and its worker, so cross-session deadlock is structurally
  impossible; per-session ordering is FIFO, except that an upstream FETCH,
  the replies to forwarded PUBLISH_NAMESPACE messages and forwarded PUBLISHes
  complete in their own tasks after the events that followed them.
- **Worker lifetime is the session's**: a session worker exists from the
  session's registration until its terminal event; its mailbox leaves the
  repository with the session, so an event for a departed session has no
  destination and is dropped by its sender.
- **One producer per track and publisher**: the track's `Creating` entry
  makes every SUBSCRIBE after the first wait for its creation instead of
  subscribing upstream again, and a publisher answering after the track
  already holds its producer is unsubscribed again, without an ingest.
- **SUBSCRIBE_OK matches egress**: the largest location advertised downstream
  is the same value the egress scheduler starts from.
- **Start Location is a lower bound**: egress delivers the first group at or
  above the start whether it arrives as an open event or is already cached;
  neither path requires the start group id itself to exist.
- **Every publisher is ingested**: one reader set per track and publisher,
  owned by its `Producer`, covering its subgroup streams and datagrams, and
  inserting into the track's single cache;
  dropping a producer ends only its own readers, and only a FIN covering a
  stream subgroup from its earliest received object completes it.
- **Egress runner lifetime is the consumer's**: a runner stops when its
  `Consumer` is removed from the track, regardless of which session worker
  removes it or when; a consumer is only ever registered while its track
  exists.
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
  transport priority is set before its header (§7.2 order: subscriber
  priority, publisher priority, group order, subgroup id), and only an update
  of the subscriber priority changes it afterwards; the control stream
  keeps the transport default and so outranks all data streams.
- **Knowledge follows objects**: evicting an object releases the knowledge at
  exactly that location.
- **Cache lifetime**: a track cache lives while referenced or until TTL
  eviction empties it with `strong_count == 1`.
- **Client-owned routes**: Redis namespace and track routes are
  registered/withdrawn only for client-origin sessions, and watched-namespace routes only while a
  client watches the namespace; relay-learned namespaces are purged locally,
  and the namespace subscriptions at other relays released,
  when the last client subscriber for the prefix leaves.
- **Relay subscriptions serve clients**: an upstream subscription to another
  relay only exists while a client of this relay watches the track, and a
  relay's SUBSCRIBE is never forwarded to a third relay.
- **A draining relay is left by its clients, not by the relay**: after
  GOAWAY the relay only waits; it closes a client session itself only at the
  drain timeout, with `GOAWAY_TIMEOUT`.

## Design direction

The relay was reshaped in November 2026 so that fewer parts carry the same
state. Three aggregates and a registry:

- `Track` owns the session↔track edges — a `Producer` per upstream
  subscription (its `moqt` handle, origin, peer and the ingest task) and a
  `Consumer` per downstream subscription (its handle, `DeliveryControl`,
  stats and the signals its egress runner lives by) — and with them the
  draft-14 §8.2 lifetime rules (see "`modules/control_plane/sequences`").
- Waits on another session's answer are state of the `Track`: an upstream
  SUBSCRIBE is a pending producer counted on the track, whose `Creating`
  state replaced the former creation serializer, and a forwarded PUBLISH a
  pending consumer. `UpstreamFetchTask` stays a task of its own: a FETCH
  fill is no edge of the track.
- The session entry keeps only session-level state (handle, token, span,
  peer, expiry, mailbox); the worker reads it from there and holds nothing of
  its own. Which edges a session has is a directory query (`tracks_fed_by`,
  `consumer_tracks`), not a second index kept in step with the tracks.
- `SessionRepository` and the directory are the registry; no object above
  them carries rules. The sequences are thin choreography over them, and the
  per-session worker is the dispatcher that reads in order, runs the
  authorization gate and performs the effects the aggregates return.

A state transition returns effects; the caller performs the I/O. Nothing
sends a control message under a `Track` lock or from a `Drop`.

## Testing conventions
Unit tests are colocated (`#[cfg(test)]`) and pin structural invariants —
e.g. worker non-blocking and terminal-event handling in
`control_plane/session_workers.rs`, largest-location resolution in `control_plane/sequences/subscribe.rs`,
eviction refcount rules in `data_plane/cache/store.rs`. Shared mocks and the
data-plane `RelayHarness` live in `test_support/`. Multi-process behaviour
(cascading relays, cache eviction, fetch, multiple publishers, dedup) lives in
the workspace-level `tests/*-e2e` suites, each driven by its `run.sh`.
