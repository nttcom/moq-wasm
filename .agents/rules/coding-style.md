# Coding Style Rules

Shared coding style rules for all agents (Claude Code, Codex). Read this before writing or modifying implementation code.

## Implementation Principles
- Follow YAGNI. Implement only what the current requirement needs.
- Do not add configuration options, abstraction layers, generic parameters, or extension points for anticipated future needs. Add them when a second concrete use case appears.
- Prefer the smallest change that satisfies the requirement over a larger refactor, unless the task explicitly asks for the refactor.

## Comments
- Keep comments to a minimum. Code that needs a comment to be readable should usually be renamed or restructured instead.
- Write a comment only for the non-obvious *why* — an invariant, a specification constraint, a workaround, or a deliberate trade-off. Do not restate what the code already says.
- Exceptions where comments are still required: the invariant behind `panic!`/`unreachable!` and the meaning of abbreviations in type declarations.
- In test code, the `// Arrange` / `// Act` / `// Assert` phase-separator comments are always allowed even though they only label the phase. See `testing.md`.

## Naming
- Follow the naming conventions of the implementation language (e.g. `snake_case` for Rust, `camelCase` for TypeScript).
- Names must represent the target concept clearly.
- Abbreviations are allowed, but when used in type declarations, document the meaning in a code-level doc comment.
- Channel-related variables must be named using the `xxx_sender` or `yyy_receiver` pattern.

## Error Handling
- Use `anyhow::Result` by default.
- Use `std::io::Result` only when explicitly instructed.
- For recoverable failures, do not use `panic!`/`unwrap`; return `Result` instead.
- `panic!`/`unwrap` is allowed during initialization/startup only when the process cannot continue safely.
- `panic!`/`unreachable!` is allowed for proven unreachable states only; in this case, add a comment that explains the invariant.
- Choose `bool` / `Option` / `Result` in this order:
  1. If the caller needs the failure reason or recovery action, use `Result`.
  2. Otherwise, if a value may be absent as a normal case, use `Option`.
  3. Otherwise, use `bool` for pure yes/no semantics.
- Use `Option` only when `None` is an expected and non-error state.
- If the return state may expand beyond true/false in the future, prefer a dedicated enum over `bool`.

## Async / Tasks
- No special naming convention for async functions.
- Background tasks must be encapsulated in a dedicated struct that owns the `JoinHandle`.
- The struct's constructor `run()` spawns the task and returns `Self`.
- If the task needs to receive commands, store an `mpsc::Sender` alongside the `JoinHandle` (actor pattern).
- Place each task file in the directory whose responsibility the task fulfills, not necessarily where it is spawned.
  - Example: `moqt/src/modules/moqt/runtime/tasks/control_message_receive_task.rs`

## Async Primitives
- In async code, use `tokio` equivalents over `std` for synchronization, file I/O, and time operations.
- `std::sync::Mutex` is acceptable only when the lock is never held across an `.await` point.

## Imports
- Use standard `use` declarations. Do not write full paths inline unless disambiguation is needed.
- When the same type name exists in both `std` and `tokio` within one file, qualify with the module path (e.g. `std::sync::Mutex`) instead of creating aliases.

## Module Structure
- Split modules and structs based on SOLID principles with functional cohesion.
- Keep each file under 300 lines. If a file exceeds this, consider splitting it.
- When creating a directory module, use a same-name `.rs` file (e.g. `foo.rs` + `foo/`) instead of `foo/mod.rs`.

## Visibility
- `pub` — only for items exported outside the crate.
- `pub(crate)` — for items shared within the crate.
- `pub(super)` — for items accessed only from the parent module.
- Private (no modifier) — everything else.

## Dependencies
- Minimize the number of external crates. Prefer existing dependencies over adding new ones.
- When adding a new crate, create an Architecture Decision Record (ADR) under `architecture_decision_record/${package_name}/` at the repository root. Use the target package's `name` value in `Cargo.toml` as `${package_name}` (e.g. `architecture_decision_record/media-streaming-format/tokio-util.md` for `shared/media-streaming-format`). Each ADR must be a separate Markdown file named after the crate. The ADR must include:
  1. What — the crate being added and its purpose.
  2. Context — the problem or requirement that motivates the addition.
  3. Alternatives — other crates or approaches considered, with trade-offs.
  4. Decision — the final choice and rationale.
- If you are unsure about ADR writing style or structure, refer to `architecture_decision_record/example/000_jwt_authentication.md` as a reference.

## Unsafe
- `unsafe` is prohibited in principle.
- Allowed only when the compiler explicitly requires it or when interfacing with FFI.
