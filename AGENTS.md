# AGENTS.md

## 1. Language
- Respond concisely and politely in the user's language.
- Write all documentation and code comments in English unless a later section defines a specific exception.

## 2. Project Scope
- This repository implements Media over QUIC Transport (MoQT), a low-latency, QUIC-based application-layer transport protocol.
- The project is organized around the following components:

| Component | Description | Related Draft |
| --- | --- | --- |
| `moqt` | Core MoQT implementation | `spec/draft-ietf-moq-transport-14.txt` |
| `relay` | MoQT relay server implementation | `spec/draft-ietf-moq-transport-14.txt` (`relay`-related sections) |
| `shared/media-streaming-format` | Content format for transporting media over MoQT | `spec/draft-ietf-moq-msf-00.txt` |

## 3. Specifications
- MoQT-related specifications are stored in the `spec` directory.
- Use only the draft listed in `Related Draft` as the authoritative specification for the component you are changing.
- Do not consult other drafts unless the task explicitly requires them.
- When implementation details are unclear, consult the relevant draft before answering questions or making code changes.
- For definitions shared by clients and servers, verify the draft text carefully and implement behavior accordingly.

## 4. Coding Style

### Naming
- Channel-related variables must be named using the `xxx_sender` or `yyy_receiver` pattern.

### Error Handling
- Use `anyhow::Result` by default.
- Use `std::io::Result` only when explicitly instructed.

### Async / Tasks
- No special naming convention for async functions.
- Background tasks must be encapsulated in a dedicated struct that owns the `JoinHandle`.
- Place each task struct in a dedicated file within the smallest relevant directory.
- The struct's constructor `run()` spawns the task and returns `Self`.
- If the task needs to receive commands, store an `mpsc::Sender` alongside the `JoinHandle` (actor pattern).

### Module Structure
- Split modules and structs based on SOLID principles with functional cohesion.
- When creating a directory module, use a same-name `.rs` file (e.g. `foo.rs` + `foo/`) instead of `foo/mod.rs`.

### Visibility
- `pub` — only for items exported outside the crate.
- `pub(crate)` — for items shared within the crate.
- `pub(super)` — for items accessed only from the parent module.
- Private (no modifier) — everything else.

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
- In the Act and Assert phases, add Japanese comments at an appropriate granularity so the test intent and verification points are easy to scan.
- If the flow of operations or the expected result is not immediately obvious, do not leave the Act and Assert phases without comments.
- When modifying tests, confirm that shared Arrange utilities still have reusable responsibilities and scope, and that the Japanese comments in Act and Assert remain sufficiently descriptive.

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