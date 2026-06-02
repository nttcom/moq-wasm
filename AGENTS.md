# AGENTS.md

## 1. Language
- Respond concisely and politely in the user's language.
- Write all documentation and code comments in English unless a later section defines a specific exception.

## 2. Project Scope
- This repository implements Media over QUIC Transport (MoQT), a low-latency, QUIC-based application-layer transport protocol.
- The project is organized around the following components.

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

## 3. Specifications
- MoQT-related specifications are stored in the `spec` directory.
- For library components listed in the draft-governed table above, use only the draft listed in `Related Draft` as the authoritative specification.
- For application and integration components, draft lookup is not required unless the task explicitly asks for specification-level alignment.
- Do not consult other drafts unless the task explicitly requires them.
- When implementation details are unclear, consult the relevant draft before answering questions or making code changes.
- For definitions shared by clients and servers, verify the draft text carefully and implement behavior accordingly.
- If ambiguity remains after consulting both the specification and this document, ask the user for clarification rather than guessing.
- Do not read the entire draft. Search for the relevant section heading or keyword to locate the needed content.

## 4. Coding Style

### Naming
- Follow the naming conventions of the implementation language (e.g. `snake_case` for Rust, `camelCase` for TypeScript).
- Names must represent the target concept clearly.
- Abbreviations are allowed, but when used in type declarations, document the meaning in a code-level doc comment.
- Channel-related variables must be named using the `xxx_sender` or `yyy_receiver` pattern.

### Error Handling
- Use `anyhow::Result` by default.
- Use `std::io::Result` only when explicitly instructed.
- Choose `bool` / `Option` / `Result` in this order:
  1. If the caller needs the failure reason or recovery action, use `Result`.
  2. Otherwise, if a value may be absent as a normal case, use `Option`.
  3. Otherwise, use `bool` for pure yes/no semantics.
- Use `Option` only when `None` is an expected and non-error state.
- If the return state may expand beyond true/false in the future, prefer a dedicated enum over `bool`.

### Async / Tasks
- No special naming convention for async functions.
- Background tasks must be encapsulated in a dedicated struct that owns the `JoinHandle`.
- The struct's constructor `run()` spawns the task and returns `Self`.
- If the task needs to receive commands, store an `mpsc::Sender` alongside the `JoinHandle` (actor pattern).
- Place each task file in the directory whose responsibility the task fulfills, not necessarily where it is spawned.
  - Example: `moqt/src/modules/moqt/runtime/tasks/control_message_receive_task.rs`

### Async Primitives
- In async code, use `tokio` equivalents over `std` for synchronization, file I/O, and time operations.
- `std::sync::Mutex` is acceptable only when the lock is never held across an `.await` point.

### Imports
- Use standard `use` declarations. Do not write full paths inline unless disambiguation is needed.
- When the same type name exists in both `std` and `tokio` within one file, qualify with the module path (e.g. `std::sync::Mutex`) instead of creating aliases.

### Module Structure
- Split modules and structs based on SOLID principles with functional cohesion.
- Keep each file under 300 lines. If a file exceeds this, consider splitting it.
- When creating a directory module, use a same-name `.rs` file (e.g. `foo.rs` + `foo/`) instead of `foo/mod.rs`.

### Visibility
- `pub` — only for items exported outside the crate.
- `pub(crate)` — for items shared within the crate.
- `pub(super)` — for items accessed only from the parent module.
- Private (no modifier) — everything else.

### Dependencies
- Minimize the number of external crates. Prefer existing dependencies over adding new ones.
- When adding a new crate, create an Architecture Decision Record (ADR) under `architecture_decision_record/${package_name}/` at the repository root. Use the target package's `name` value in `Cargo.toml` as `${package_name}` (e.g. `architecture_decision_record/media-streaming-format/tokio-util.md` for `shared/media-streaming-format`). Each ADR must be a separate Markdown file named after the crate. The ADR must include:
  1. What — the crate being added and its purpose.
  2. Context — the problem or requirement that motivates the addition.
  3. Alternatives — other crates or approaches considered, with trade-offs.
  4. Decision — the final choice and rationale.
- If you are unsure about ADR writing style or structure, refer to `architecture_decision_record/example/000_jwt_authentication.md` as a reference.

### Unsafe
- `unsafe` is prohibited in principle.
- Allowed only when the compiler explicitly requires it or when interfacing with FFI.

## 5. Commands
- Build: `cargo build`
- Test: `cargo test`
- Lint (Rust): `cargo clippy && cargo fmt --check`
- Lint (JavaScript): `npx prettier --check`
- Wasm: `wasm-pack build bindings/wasm`
- Relay: `cargo run --bin relay`

## 6. Testing Guidelines
- Structure test code using the Arrange / Act / Assert pattern.
- Extract Arrange-phase setup into shared utility files; do not duplicate setup across test files.
- In the Act and Assert phases, add comments at an appropriate granularity so the test intent and verification points are easy to scan.
- If the flow of operations or the expected result is not immediately obvious, do not leave the Act and Assert phases without comments.
- When modifying tests, confirm that shared Arrange utilities still have reusable responsibilities and scope, and that the comments in Act and Assert remain sufficiently descriptive.

## 7. Logging
Always use the `tracing` crate for log output (e.g. `tracing::info!`, `tracing::debug!`).
Follow the log level guidelines below.

| Level | Role | Example Events |
| --- | --- | --- |
| TRACE | Raw network data. Disabled in normal operation. | · Raw bytes sent/received on a stream<br>· QUIC frame-level events |
| DEBUG | Detailed state changes for debugging. | · MOQT message contents (parsed fields)<br>· Stream open/close events<br>· Session state transitions |
| INFO | Key milestones to monitor in production. | · Connection established/closed<br>· Subscribe/Publish started or completed<br>· Session start and teardown |
| WARN | Recoverable issues that require attention. | · Retrying after a transient connection error<br>· Received an unexpected but non-fatal message type<br>· Stream closed by peer earlier than expected |
| ERROR | Fatal failures requiring intervention. | · Connection failed after maximum retries<br>· Received a malformed or unrecognized MOQT message<br>· Authentication or TLS handshake failure |

## 8. Git Conventions
- Follow Conventional Commits: `type(scope): description`
  - Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`
  - Scope: use the component name (`moqt`, `relay`, `wasm`, `live-ingest`, `onvif`, `msf`, `packages`)
- One commit per logical change. If the description requires "and", split into multiple commits.
- PR titles must clearly describe what was changed.
- PR descriptions must be written in Japanese and include:
  1. What feature or fix is being addressed.
  2. What changes were made.
  3. Why — the intent behind the changes.