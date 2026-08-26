# AGENTS.md

## 1. Language
- Respond concisely and politely in the user's language.
- Write all documentation and code comments in English unless a later section defines a specific exception.

## 2. Project Scope
- This repository implements Media over QUIC Transport (MoQT), a low-latency, QUIC-based application-layer transport protocol.

Library components (draft-governed):

| Component | Description | Related Draft |
| --- | --- | --- |
| `moqt` | Core MoQT protocol implementation | `spec/draft-ietf-moq-transport-14.txt` |
| `relay` | MoQT relay server, extending `moqt` with server-specific logic | `spec/draft-ietf-moq-transport-14.txt` (`relay`-related sections) |
| `shared/media-streaming-format` | Object format for content transported over MoQT | `spec/draft-ietf-moq-msf-00.txt` |
| `shared/packages` | Low-overhead container used internally by `media-streaming-format` | `spec/draft-ietf-moq-msf-00.txt` |

Application and integration components (draft reference is normally not required):

| Component | Description |
| --- | --- |
| `bindings/wasm` | WebAssembly bindings to use `moqt` from the browser |
| `bridges/live-ingest` | Bridge converting RTMP/SRT streams into MoQT |
| `bridges/onvif` | Bridge ingesting ONVIF camera streams into MoQT |
| `examples/` | Usage examples and test clients |

- `moqt` is the central crate — all other crates depend on it. Changes to `moqt` affect the entire workspace.

### Architecture Documents
- Architecture documents live at `architecture_decision_record/${package_name}/architecture.md` (currently `moqt` and `relay`).
- Before making structural changes to a component (module layout, layering, task/channel topology), read its architecture document first.
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
- Lint (Rust): `cargo clippy && cargo fmt --check`
- Lint (JavaScript): `npx prettier --check`
- Wasm: `wasm-pack build bindings/wasm`
- Relay: `cargo run --bin relay`
- E2E Test (media): `node scripts/run-media-e2e.mjs`
- E2E Test (call): `node scripts/run-call-e2e.mjs`
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

## 8. Git Conventions
- Commit messages must be written in English.
- Follow Conventional Commits: `type(scope): description`
  - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`
  - Scope: use the component name (`moqt`, `relay`, `wasm`, `live-ingest`, `onvif`, `msf`, `packages`)
- One commit per logical change. If the description requires "and", split into multiple commits.
- PR titles and descriptions must be written in Japanese; descriptions follow `.github/pull_request_template.md`.
- When creating a pull request, follow the skill in `.agents/skills/create-pull-request/SKILL.md`.
