# AGENTS.md

## 1. Language
- Reply concisely and politely in the user's language.
- Write documentation and code comments in English unless noted otherwise below.

## 2. Project Scope
This repository implements Media over QUIC Transport (MoQT), a low-latency QUIC-based transport protocol.

### Workspace Layout
- `services/` — standalone deployed processes (servers and bridges: `relay`, `vts`, `ingest-gateway`).
- `crates/` — libraries linked by other crates and SDKs (`moqt`, `broadcast`), including libraries with an optional CLI (`auth-token`).
- `sdk/` — SDKs and plugins released to external users:
  - `sdk/js`: crate `wasm`, npm package under `js/`.
  - `sdk/python`: crate `python` and Python package.
  - `sdk/rust`: crate `moq-wasm` (matches the other SDKs' install name for git dependencies).
  - `sdk/gstreamer`: crate `gst-plugin-moqt` (GStreamer `moqtsink` plugin).
- `moqt` is the central crate; all other crates depend on it, so changes there affect the entire workspace.

### Crate Naming
- Omit any `moqt-` prefix (the workspace is the namespace). Binaries take the package name.
- Pick a name in order:
  1. Implements a spec → spec name (`moqt`, `msf`).
  2. External ecosystem convention → follow it (`gst-plugin-moqt`).
  3. Otherwise → 1–2 kebab-case role words (`relay`, `broadcast`, `ingest-gateway`, `egress-gateway`).
- Abbreviate only if already used in code, flags, or env vars (`msf`, `vts`); otherwise spell out (`auth-token`).

### Draft-Governed Components

| Component | Description | Related Draft |
| --- | --- | --- |
| `moqt` | Core MoQT protocol | `docs/spec/draft-ietf-moq-transport-14.txt`; `docs/spec/draft-ietf-moq-transport-18.txt` (`modules/moqt/version/draft18`) |
| `relay` | MoQT relay server extending `moqt` | `docs/spec/draft-ietf-moq-transport-14.txt` (relay sections) |
| `crates/msf` | Object and catalog format over MoQT | `docs/spec/draft-ietf-moq-msf-02.txt`, `docs/spec/draft-ietf-moq-cmsf-01.txt` |
| `crates/mediapack` | Container mux/demux (MPEG-TS, FLV, fMP4, LOC), progressive MP4 index, H.264/AAC helpers; `moqt` feature maps LOC properties to MoQT extension headers | `docs/spec/draft-ietf-moq-loc-04.txt` (`loc` module only) |
| `crates/broadcast` | MSF broadcast writer/reader (`mediapack` ↔ catalog, LOC/CMAF tracks, media timeline, publish policy, track selection) for `ingest-gateway`, `gst-plugin-moqt`, `egress-gateway`, `sip-gateway`, and `recorder`; extends `msf` and `moqt` with original-publisher logic | `docs/spec/draft-ietf-moq-msf-02.txt` (§5 Catalog, §7 Media timeline, §11 Workflow), `docs/spec/draft-ietf-moq-cmsf-01.txt` (§3.2 switching sets) |
| `crates/robotics` | Robot teleoperation, cloud policies, and data collection (`state`, `control`, `episode`, `action` tracks; `Robot`/`Arbiter` dead-man arbitration; `Driver` writer); extends `broadcast` and `moqt` | `docs/spec/draft-ietf-moq-msf-02.txt` (§8 Event Timeline) |

All other crates under `services/`, `crates/`, `sdk/`, and `examples/` are application/integration components (see each crate's `Cargo.toml` `description` and `docs/architecture/<package_name>/`); draft lookup is normally not required.

### Architecture Documents
- Located at `docs/architecture/<package_name>/` (`<package_name>` = `Cargo.toml` `name`). Main file: `architecture.md` (`sdk.md` for SDKs, `browser-sdk.md` for `wasm`). ADR-only directories have no main doc.
- Read the architecture doc before structural changes (module layout, layering, task/channel topology).
- Update it in the same change when altering design intent, module boundaries, runtime flow, or key invariants.

## 3. Specifications
- For draft-governed components, treat **only** the listed `Related Draft` as authoritative unless explicitly asked otherwise.
- For other components, skip draft lookup unless spec alignment is explicitly requested.
- Search drafts by section heading or keyword; do not read entire drafts.
- Consult the draft before answering or coding when protocol details are unclear.
- Ask the user how far to implement RFC 2119 requirements (`MUST`, `SHOULD`, `MAY`) before proceeding.
- If still ambiguous after consulting the draft and this file, ask the user rather than guessing.

## 4. Coding Style
Read `.agents/rules/coding-style.md` before your first code edit if not already in context.

@.agents/rules/coding-style.md

## 5. Commands
- Build: `cargo build`
- Test: `cargo test` (run `cargo test -p <package_name>` after changes to verify no regressions)
- Lint (Rust): `cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
- Lint (Rust, wasm32): `cargo clippy -p wasm -p moqt -p broadcast -p telemetry -p telemetry-report -p mediapack -p msf --target wasm32-unknown-unknown -- -D warnings`
- Lint (JavaScript): `npx --prefix examples/browser prettier --check "examples/browser/**/*.{js,jsx,ts,tsx,json,css,md}" "sdk/js/js/**/*.{js,ts,json,css,md}" --ignore-path .gitignore --ignore-path examples/browser/.prettierignore --ignore-path sdk/js/js/.prettierignore`
- Test (browser unit): `npm test --workspaces`
- Test (Python): `cd sdk/python && uv run pytest` (and `uv run pytest` in `examples/python/<project>`)
- Wasm / JS SDK: `npm run wasm -w moq-wasm && npm run build -w moq-wasm`
- Console web: `npm --prefix services/console/web run format:check && npm --prefix services/console/web run lint && npm --prefix services/console/web run build`
- Relay: `cargo run --bin relay`
- Mutation test (branch diff): `git diff origin/master... > /tmp/pr.diff && cargo mutants --in-diff /tmp/pr.diff -p <package_name>`
- E2E tests: see `tests/README.md` (`./tests/<suite>/run.sh` for relay/service suites, `node tests/browser-e2e/run-*-e2e.mjs` for browser suites)

## 6. Testing Guidelines
Read `.agents/rules/testing.md` before your first code edit if not already in context.

@.agents/rules/testing.md

## 7. Logging
Always use `tracing` (`tracing::info!`, `tracing::debug!`, etc.).

| Level | Role | Examples |
| --- | --- | --- |
| TRACE | Raw network data (disabled in normal operation) | Raw stream bytes, QUIC frame events |
| DEBUG | Detailed state transitions for debugging | Parsed MoQT messages, stream open/close, state transitions |
| INFO | Production milestones | Connection open/close, subscribe/publish start/end, session lifecycle |
| WARN | Recoverable issues | Transient retry, unexpected non-fatal message, early peer stream close |
| ERROR | Fatal failures | Connection failed after retries, malformed/unknown MoQT message, auth/TLS failure |

### OTLP Rules (`INFO` and above)
Logs export over OTLP without inheriting span fields:
- **One event = one `INFO` line**: writing a request (`SUBSCRIBE`, `FETCH`, `PUBLISH`, …) and writing its response (`*_OK`, `*_ERROR`) are separate events. Intermediate steps, cleanup, and `Drop` are `DEBUG`.
- **Fixed message string**: pass every value as a structured field (`tracing::info!(%error, request_id, "SUBSCRIBE_ERROR received")`, never `"... {}"`).
- **Self-contained fields**: include the fields identifying the entity so the log is searchable without its span.

### Spans
- Long-lived spans (track, session) root their own trace and link to the starting request's span (child spans block parent trace export until closed).
- Relay request spans continue the client's trace when the request carries `TRACE_CONTEXT`.

### Field Vocabulary
Use only field names from this table; add new names here before using them:

| Field | Meaning |
| --- | --- |
| `session_id` | Session the event is about |
| `publisher_session_id` / `subscriber_session_id` | Both sessions of a track when both are involved |
| `session_peer` | `client` or `relay` |
| `app_id` | Application of the session's token |
| `client_id` | Browser page load ID sent with telemetry |
| `request_id` | Draft Request ID (not `subscribe_id`) |
| `message_type` | Control message type when not named in the message |
| `track_namespace` / `track_name` | Track identifier (not `track_key` or `namespace`) |
| `track_namespace_prefix` | `SUBSCRIBE_NAMESPACE` prefix |
| `media_source` | Broadcast source in a browser, named by video track (`camera`, `screen`, …) |
| `ready_state` | `MediaStreamTrack.readyState` (`live` or `ended`) |
| `call_id` / `direction` / `call_end_reason` | SIP gateway call ID, `outbound`/`inbound`, and end reason (`hangup`, `timeout`, `limit`, `gateway`, or SIP status) |
| `sip_call_id` / `sip_status` | SIP dialog Call-ID and response status |
| `bot_id` / `bot_type` / `bot_state` | Bot ID, type (`recorder`, …), and reported state (`running`, `finished`, …) |
| `track_alias` | Track alias on the session |
| `group_id` / `subgroup_id` / `object_id` | Object Group ID, Subgroup ID, and Object ID |
| `forward` | Forward State set by a message or held by a subscription |
| `new_group_request` | `NEW_GROUP_REQUEST` value: largest known Group ID + 1, or `0` |
| `supported_versions` / `selected_version` | Versions offered by `CLIENT_SETUP` and picked by `SERVER_SETUP` |
| `error` | Failure, formatted as `%error` or `?error` |
| `is_relay` | Whether the token's app is a relay |
| `outcome` | Request result (`ok`, `rejected`, `timeout`, `failed`) or service call result (`relay.vts.verify`: `verified`/`unauthorized`/`unavailable`; `vts.request`: `verified`/`malformed_token`/`unknown_app`/`invalid_signature`) |
| `relay_hostname` | Relay host on every root span (carried on each stdout line) |
| `url` / `fallback_url` | Connection URL and `wss://` fallback URL |
| `transport` | Opened transport: `QUIC`, `WebTransport`, or `WebSocket` |
| `remote_address` / `previous_remote_address` | Peer address (or SIP trunk TLS address) and the address replaced on migration |
| `duration_ms` | Milliseconds from writing a request to receiving its response |
| `wasm_load_wait_ms` | Milliseconds `MoqSession.connect` waited for wasm to load |
| `driver` | Namespace driving a robot relative to its room (`operators/<id>`, `bots/policy`); omitted when idle |
| `hold_reason` | Why a robot holds still: `stale` (no driver or no fresh action) or `session` (relay session ended) |
| `control_reason` | Robot `control` record reason: `action`, `left`, `stale`, or `episode` |
| `episode_id` | Robot's open episode ID |
| `loop_latency_ms` | `now − basis` of an applied robot action (ms from observation to arrival on robot clock) |

## 8. Git Conventions
- **Commits**: English, Conventional Commits `type(scope): description`.
  - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`.
  - Scope: crate or component name (`moqt`, `relay`, `wasm`, `python`, `moq-wasm`, `broadcast`, `ingest-gateway`, `egress-gateway`, `msf`, `mediapack`, etc.).
  - One logical change per commit (if the message needs "and", split it).
  - Squash-merged commits on `master` take the Japanese PR title; branch commits stay English.
- **PRs**: Japanese title and body, following `.github/pull_request_template.md` and `.agents/skills/create-pull-request/SKILL.md`.
- **Public repo**: never reference private repos or internal trackers (URLs, repo names, issue/PR numbers) in code, docs, commits, or PRs. Link only from private to public.
- **SDK releases**:
  - Browser (`moq-wasm`): `.agents/skills/release-sdk/SKILL.md`
  - Python (`sdk/python`): `.agents/skills/release-python-sdk/SKILL.md`
  - Rust (`sdk/rust`): `.agents/skills/release-rust-sdk/SKILL.md`
