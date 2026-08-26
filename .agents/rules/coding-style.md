# Coding Style Rules

Shared by all agents (Claude Code, Codex).

## Implementation Principles
- Follow YAGNI: implement only what the current requirement needs. Do not add configuration options, abstraction layers, generic parameters, or extension points for anticipated future needs — add them when a second concrete use case appears.
- Prefer the smallest change that satisfies the requirement over a larger refactor, unless the task explicitly asks for the refactor.
- Before fixing a bug, investigate the cause first: reproduce it and explain the mechanism. A retry, sleep, widened timeout, defensive check, or call-site special case without a stated mechanism is a symptom patch, not a fix.
- If the cause lies in a lower layer (typically the `moqt` crate), fix it there instead of working around it in the caller. The workaround becomes load-bearing and hides the bug from the next caller.
- Refactor, as part of the same task rather than leaving a TODO, when any of the following applies:
  - A function takes 4+ arguments, a call site passes the same 3+ values into multiple functions, or the same tuple is returned across modules — introduce a struct.
  - A fix needs the same edit in N places — reshape so it is one place first, then fix. Prefer extending an existing primitive over adding a parallel one-off, and generalizing a helper over copying it.
  - The task can be solved either by patching around an awkward internal shape or by fixing the shape — fix the shape. Do not preserve an awkward shape just to avoid churn; this applies to internal code as well as public APIs.
- When a fix requires a refactor, open the refactor as its own PR first, then stack the fix that builds on it as a stacked PR on top of the refactor PR — do not mix the two in one PR.

## Comments
- Do not write comments. Before adding or keeping one, apply this test: can the reader derive the information from the code itself (names, types, control flow, test names)? If yes, the comment is prohibited — this applies to all code, not only tests: production code, mocks, and doc comments alike. A doc comment that restates the item's name or signature fails the test.
- When code seems to need a comment, make the code convey the information instead — rename the function or variable, split the function, or restructure — and then delete the comment. This refactoring is part of the change that removes the comment; do not keep a comment to avoid the refactoring.
- The only information that may stay as a comment is what code cannot express: an invariant, a specification constraint, a workaround, or a deliberate trade-off that affects current behavior.
- Change history, past implementations, migration notes, review discussion, TODO speculation, and plans for future work do not belong in the code body. Put them in the commit message, the pull request, or an architecture document.
- A comment is required in three cases only: the invariant behind `panic!`/`unreachable!`, the meaning of abbreviations in type declarations, and the bare `// Arrange` / `// Act` / `// Assert` labels in tests (see `testing.md`).

## Naming
- Follow the naming conventions of the implementation language (e.g. `snake_case` for Rust, `camelCase` for TypeScript).
- Names must represent the target concept clearly.
- Abbreviations are allowed.
- Channel-related variables must be named using the `xxx_sender` or `yyy_receiver` pattern.

## Error Handling
- Use `anyhow::Result` by default.
- Use `std::io::Result` only when explicitly instructed.
- For recoverable failures, do not use `panic!`/`unwrap`; return `Result` instead.
- `panic!`/`unwrap` is allowed during initialization/startup only when the process cannot continue safely.
- `panic!`/`unreachable!` is allowed for proven unreachable states only.
- Choose `bool` / `Option` / `Result` in this order:
  1. If the caller needs the failure reason or recovery action, use `Result`.
  2. Otherwise, if a value may be absent as a normal case, use `Option`.
  3. Otherwise, use `bool` for pure yes/no semantics.
- If the return state may expand beyond true/false in the future, prefer a dedicated enum over `bool`.

## Async / Tasks
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
- Keep each file under 300 lines.
- When creating a directory module, use a same-name `.rs` file (e.g. `foo.rs` + `foo/`) instead of `foo/mod.rs`.

## Visibility
- `pub` — only for items exported outside the crate.
- `pub(crate)` — for items shared within the crate.
- `pub(super)` — for items accessed only from the parent module.
- Private (no modifier) — everything else.

## Dependencies
- Minimize the number of external crates. Prefer existing dependencies over adding new ones.
- When adding a new crate, create an Architecture Decision Record (ADR) at `architecture_decision_record/${package_name}/<crate>.md`, where `${package_name}` is the package's `name` in `Cargo.toml` (e.g. `architecture_decision_record/media-streaming-format/tokio-util.md`). The ADR must cover:
  1. What — the crate being added and its purpose.
  2. Context — the problem or requirement that motivates the addition.
  3. Alternatives — other crates or approaches considered, with trade-offs.
  4. Decision — the final choice and rationale.
- For ADR style, see `architecture_decision_record/example/000_jwt_authentication.md`.

## Unsafe
- `unsafe` is prohibited in principle.
- Allowed only when the compiler explicitly requires it or when interfacing with FFI.
