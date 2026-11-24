# AGENTS.md

## 1. Language
- Respond concisely and politely in the user's language.
- Write all documentation and code comments in English unless a later section defines a specific exception.

## 2. Project Scope
- This repository implements Media over QUIC Transport (MoQT), a low-latency, QUIC-based application-layer transport protocol.
- Every Rust crate lives under `services/`, `crates/` or `sdk/`. Architecture documents live under `docs/architecture/<package name>/`, where `<package name>` is the `name` in the crate's `Cargo.toml`.
  - `services/` holds the crates whose product is a process that is deployed or run on its own: servers and bridges (`relay`, `vts`, `ingest-gateway`).
  - `crates/` holds the code other crates, the SDKs or GStreamer link against: libraries and plugins (`moqt`, `broadcast`, `gst-plugin-moqt`). A library that also ships a small CLI behind a feature (`auth-token`) stays here.
  - `sdk/` holds the SDKs applications install, one directory per language: `sdk/js` (the crate `wasm` and the npm package under `js/`), `sdk/python` (the crate `python` and the Python package) and `sdk/rust` (the crate `moq-wasm`, named as the other two SDKs are installed, since a git dependency is named by its package).
- Crate names carry no `moqt-` prefix: the workspace is the namespace. Pick the name with this test:
  1. The crate implements a specification → use the specification's name (`moqt`, `msf`).
  2. An external ecosystem dictates the form → follow it (`gst-plugin-moqt`).
  3. Otherwise → name the role in one or two kebab-case words (`relay`, `broadcast`, `ingest-gateway`, `egress-gateway`).
  Use an abbreviation only when it is already the identifier in code, flags or environment variables (`msf`, `vts`); otherwise spell it out (`auth-token`). Binaries take the package name.

Library components (draft-governed):

| Component | Description | Related Draft |
| --- | --- | --- |
| `moqt` | Core MoQT protocol implementation | `docs/spec/draft-ietf-moq-transport-14.txt`; `docs/spec/draft-ietf-moq-transport-18.txt` for `modules/moqt/version/draft18` |
| `relay` | MoQT relay server, extending `moqt` with server-specific logic | `docs/spec/draft-ietf-moq-transport-14.txt` (`relay`-related sections) |
| `crates/msf` | Object format for content transported over MoQT | `docs/spec/draft-ietf-moq-msf-02.txt`, `docs/spec/draft-ietf-moq-cmsf-01.txt` |
| `crates/mediapack` | Container demuxers/muxers (MPEG-TS, FLV, fMP4, LOC), a progressive MP4 sample index and H.264/AAC bitstream helpers; the `moqt` feature maps LOC properties to MoQT extension headers | `docs/spec/draft-ietf-moq-loc-04.txt` (`loc` module only) |
| `crates/broadcast` | Builds an MSF broadcast from `mediapack` media events: the catalog, LOC/CMAF tracks, media timeline and publishing policy shared by `ingest-gateway` and `gst-plugin-moqt`, and the reader that turns a broadcast back into media events for `egress-gateway` and `sip-gateway`, with the track selection `recorder` shares; extends `msf` and `moqt` with original-publisher logic | `docs/spec/draft-ietf-moq-msf-02.txt` (§5 Catalog, §7 Media timeline, §11 Workflow), `docs/spec/draft-ietf-moq-cmsf-01.txt` (§3.2 switching sets) |
| `crates/robotics` | Robot teleoperation, cloud policies and data collection over MoQT: the records of the robot's `state`, `control` and `episode` tracks and the drivers' `action` tracks, their catalog entries and priorities, `Robot`, the robot's side: arbitration and the dead man (`Arbiter`) over the room's MoQT tracks, and `Driver`, a driver's `action` writer; extends `broadcast` and `moqt` | `docs/spec/draft-ietf-moq-msf-02.txt` (§8 Event Timeline) |

All other crates under `services/`, `crates/`, `sdk/`, and `examples/` are application and integration components (see each crate's `Cargo.toml` `description` and `docs/architecture/<package name>/`); draft lookup is normally not required for them.

- `moqt` is the central crate — all other crates depend on it. Changes to `moqt` affect the entire workspace.

### Architecture Documents
- Architecture documents live under `docs/architecture/${package_name}/`. Most crates have `architecture.md`; the SDKs use `sdk.md`, and `wasm` uses `browser-sdk.md`. A directory that holds only ADRs has no architecture document.
- Before making structural changes to a component (module layout, layering, task/channel topology), read its architecture document if it has one.
- When a change alters the design intent, module boundaries, runtime flow, or key invariants described in an architecture document, update that document in the same change.

## 3. Specifications
- For library components listed in the draft-governed table above, use only the draft listed in `Related Draft` as the authoritative specification. Do not consult other drafts unless the task explicitly requires them.
- For application and integration components, draft lookup is not required unless the task explicitly asks for specification-level alignment.
- Do not read the entire draft. Search for the relevant section heading or keyword to locate the needed content.
- When implementation details are unclear, consult the relevant draft before answering questions or making code changes.
- When the draft uses MUST, SHOULD, or MAY (RFC 2119 keywords), ask the user how far to implement before proceeding.
- If ambiguity remains after consulting both the specification and this document, ask the user for clarification rather than guessing.

## 4. Coding Style
- Coding style rules live in `.agents/rules/coding-style.md`, shared by all agents. If it is not already in your context, read it before your first code edit.

@.agents/rules/coding-style.md

## 5. Commands
- Build: `cargo build`
- Test: `cargo test`
- Lint (Rust): `cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo fmt --all -- --check`
- Lint (Rust, wasm32): `cargo clippy -p wasm -p moqt -p broadcast -p telemetry -p telemetry-report -p mediapack -p msf --target wasm32-unknown-unknown -- -D warnings`
- Lint (JavaScript): `npx --prefix examples/browser prettier --check "examples/browser/**/*.{js,jsx,ts,tsx,json,css,md}" "sdk/js/js/**/*.{js,ts,json,css,md}" --ignore-path .gitignore --ignore-path examples/browser/.prettierignore --ignore-path sdk/js/js/.prettierignore`
- Test (browser unit): `npm test --workspaces`
- Test (Python): `cd sdk/python && uv run pytest` (and `uv run pytest` in `examples/python/<project>`)
- Wasm / JS SDK: `npm run wasm -w moq-wasm && npm run build -w moq-wasm`
- Console web: `npm --prefix services/console/web run format:check && npm --prefix services/console/web run lint && npm --prefix services/console/web run build`
- Relay: `cargo run --bin relay`
- Mutation test (the lines a branch changes): `git diff origin/master... > /tmp/pr.diff && cargo mutants --in-diff /tmp/pr.diff -p <package_name>`
- E2E tests: see `tests/README.md` (`./tests/<suite>/run.sh` for relay/service suites, `node tests/browser-e2e/run-*-e2e.mjs` for browser suites)
- After making changes, run `cargo test -p <package_name>` for the affected package to verify no regressions.

## 6. Testing Guidelines
- Testing rules live in `.agents/rules/testing.md`, shared by all agents. Unit tests are colocated with the implementation, so read it before your first code edit as well.

@.agents/rules/testing.md

## 7. Logging
Always use the `tracing` crate for log output (e.g. `tracing::info!`, `tracing::debug!`).

| Level | Role | Example Events |
| --- | --- | --- |
| TRACE | Raw network data. Disabled in normal operation. | · Raw bytes sent/received on a stream<br>· QUIC frame-level events |
| DEBUG | Detailed state changes for debugging. | · MOQT message contents (parsed fields)<br>· Stream open/close events<br>· Session state transitions |
| INFO | Key milestones to monitor in production. | · Connection established/closed<br>· Subscribe/Publish started or completed<br>· Session start and teardown |
| WARN | Recoverable issues that require attention. | · Retrying after a transient connection error<br>· Received an unexpected but non-fatal message type<br>· Stream closed by peer earlier than expected |
| ERROR | Fatal failures requiring intervention. | · Connection failed after maximum retries<br>· Received a malformed or unrecognized MOQT message<br>· Authentication or TLS handshake failure |

Logs and spans are exported over OTLP, where a log does not inherit its span's fields. For each INFO or higher log:
- One event gets one INFO line. Writing a request (SUBSCRIBE, FETCH, PUBLISH, …) is an event of its own, so a request that is never answered still shows. Writing its response (`*_OK`, `*_ERROR`) is one too, so the side that answers shows when and how it answered. Any other step inside a request, a cleanup or a `Drop` is DEBUG.
- The message is a fixed string; every value is a field (`tracing::info!(%error, request_id, "SUBSCRIBE_ERROR received")`, not `"... {}"`).
- It carries the fields that identify what it is about, so it can be found without its span.

Name fields from this vocabulary and add a name to it before using a new one:

| Field | Meaning |
| --- | --- |
| `session_id` | The session the event is about |
| `publisher_session_id` / `subscriber_session_id` | The two sessions of a track, when both are involved |
| `session_peer` | `client` or `relay` |
| `app_id` | The application of the session's token |
| `client_id` | A browser page load, generated by the page and sent with its telemetry |
| `request_id` | The draft's Request ID (not `subscribe_id`) |
| `message_type` | The control message a log is about, when the log cannot name it in its message |
| `track_namespace` / `track_name` | The track (not `track_key` or `namespace`) |
| `track_namespace_prefix` | A SUBSCRIBE_NAMESPACE prefix |
| `media_source` | The source of a broadcast a browser plays or publishes, named by its video track (`camera`, `screen`, …) |
| `ready_state` | A `MediaStreamTrack`'s `readyState` (`live` or `ended`) when one of its events is logged |
| `call_id` / `direction` / `call_end_reason` | A phone call the SIP gateway placed or answered, whether it is `outbound` or `inbound`, and why it ended (`hangup`, `timeout`, `limit`, `gateway` or the SIP status) |
| `sip_call_id` / `sip_status` | The SIP Call-ID of a call's dialog, and the status of a SIP response |
| `bot_id` / `bot_type` / `bot_state` | A bot the bot service launched, its type (`recorder`, …), and the state it reported (`running`, `finished`, …) |
| `track_alias` | The track alias on the session the log is about |
| `group_id` / `subgroup_id` / `object_id` | The Group ID, Subgroup ID and Object ID of the object the event is about |
| `forward` | The Forward State a message sets or a subscription is in |
| `new_group_request` | The value of a NEW_GROUP_REQUEST: the largest Group ID its sender knows plus 1, or 0 |
| `supported_versions` / `selected_version` | The MoQT versions a CLIENT_SETUP offers, and the one its SERVER_SETUP picks |
| `error` | The failure, as `%error` or `?error` |
| `is_relay` | Whether a token's app is a relay |
| `outcome` | How a MoQT request ended (`ok`, `rejected`, `timeout` or `failed`), or how a call between services ended (`relay.vts.verify`: `verified`, `unauthorized` or `unavailable`; `vts.request`: `verified`, `malformed_token`, `unknown_app` or `invalid_signature`) |
| `relay_hostname` | The relay's host, on every span that roots a trace, so each stdout line carries it |
| `url` | The URL a client connects to |
| `fallback_url` | The `wss://` URL a client falls back to when `url` cannot be opened |
| `transport` | The transport a connection opened: `QUIC`, `WebTransport` or `WebSocket` |
| `remote_address` / `previous_remote_address` | The peer's address on a session's connection (or a SIP trunk's, on its TLS connection), and the one it replaced when the peer migrated |
| `duration_ms` | Milliseconds from writing a request to receiving its response |
| `wasm_load_wait_ms` | Milliseconds a browser's `MoqSession.connect` waited for the wasm module to load |
| `driver` | The namespace driving a robot, relative to its room (`operators/<id>`, `bots/policy`); absent while nobody drives |
| `hold_reason` | Why a robot holds still: `stale` (no fresh action from its driver, or no driver) or `session` (its relay session ended) |
| `control_reason` | The `reason` of a robot's `control` record: `action`, `left`, `stale` or `episode` |
| `episode_id` | The id of a robot's open episode |
| `loop_latency_ms` | `now − basis` of an action a robot applied: milliseconds from the observation it answers to its arrival, on the robot's clock |

A span that lives longer than the request that starts it (a track, a session) roots its own trace and links to the request's span: a span closes only after its children, so a long-lived child would hold its parent's trace unexported. A relay's request span continues the client's trace when the request carries `TRACE_CONTEXT`.

## 8. Git Conventions
- Commit messages must be written in English.
- A squash-merged commit on `master` takes the Japanese PR title as its subject; the English rule applies to the commits you write on a branch.
- Follow Conventional Commits: `type(scope): description`
  - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`
  - Scope: use the component or crate name (e.g. `moqt`, `relay`, `wasm`, `python`, `moq-wasm`, `broadcast`, `ingest-gateway`, `egress-gateway`, `msf`, `mediapack`)
- One commit per logical change. If the description requires "and", split into multiple commits.
- PR titles and descriptions must be written in Japanese; descriptions follow `.github/pull_request_template.md`.
- This repository is public. Do not reference private repositories or internal trackers in code, documentation, commits, or pull requests: no URLs, repository names, or issue/PR numbers. Link from the private side to this repository, never the reverse.
- When creating a pull request, follow the skill in `.agents/skills/create-pull-request/SKILL.md`.
- When releasing the browser SDK (`moq-wasm`), follow the skill in `.agents/skills/release-sdk/SKILL.md`.
- When releasing the Python SDK (`moq-wasm` in `sdk/python`), follow the skill in `.agents/skills/release-python-sdk/SKILL.md`.
- When releasing the Rust SDK (`moq-wasm` in `sdk/rust`), follow the skill in `.agents/skills/release-rust-sdk/SKILL.md`.
