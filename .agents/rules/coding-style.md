# Coding Style Rules

Shared by all agents.

## Implementation Principles
- **YAGNI**: implement only what the current requirement needs. Add options, abstractions, generics, or extension points only on a second concrete use case.
- **Minimal diff**: prefer the smallest change that meets the requirement unless refactoring is explicitly requested.
- **Out-of-scope issues**: record unrelated bugs or design issues via the `todo-add` skill; do not fix them inline or only mention them.
- **Root cause first**: reproduce a bug and explain its mechanism before fixing it. Retries, sleeps, wider timeouts, defensive checks, or call-site special cases without a stated mechanism are symptom patches, not fixes.
- **Refactor in-task** (rather than leaving a TODO) when:
  - 3+ identical values are passed into multiple functions or the same tuple crosses modules → introduce a struct.
  - A fix repeats in N places → consolidate first, then fix; extend existing primitives or generalize helpers instead of copying.
  - Fixing an awkward internal shape beats patching around it (applies to internal and public APIs).
- **Stacked PRs**: when a fix needs a refactor, open the refactor PR first and stack the fix PR on top—never mix both in one PR.

## Lower Layers
Lower layers are shared crates: `moqt`, `broadcast`, `msf`, `mediapack`, `sdk/js`, `sdk/python`, `sdk/rust`.
- Fix lower-layer bugs in the lower layer, not with caller workarounds.
- When a clean caller design needs a missing lower-layer capability, propose adding it (state signature, default behavior, and caller benefit) and wait for approval rather than bending the caller around the gap. What the task needs satisfies YAGNI; extra options still wait for a second use case.
- Open the lower-layer PR first and stack the caller PR on top.

## Comments
- Add doc comments to crate-exported items when the signature does not show failure conditions, required call order, or the implemented draft section.
- Comment invariants behind `panic!`/`unreachable!` and abbreviation meanings in type declarations.
- Keep change history, past implementations, migration notes, review discussion, TODO speculation, and future plans out of code (put them in commits, PRs, or architecture docs).
- Added comments are checked for redundancy at PR time (`.agents/skills/create-pull-request/SKILL.md`).

## Naming
- Follow language conventions (`snake_case` in Rust, `camelCase` in TypeScript); names must clearly reflect the concept. Abbreviations are allowed.
- Channels: `*_sender` and `*_receiver`.
- MoQT terms: use draft terminology. A track started via `PUBLISH` is a subscription (draft-14 §5.1) and a publisher serves a track, so never name MoQT state `Publication`. Application-facing APIs may use `Publication`.

## Error Handling
- Default to `anyhow::Result`.
- Use a `thiserror` enum when callers branch on failure kinds (instead of `downcast_ref` or string matching); mark crate-exported enums `#[non_exhaustive]`.
- Use `std::io::Result` only when explicitly instructed.
- Return `Result` for recoverable failures—no `panic!`/`unwrap`.
- Allow `panic!`/`unwrap` only at startup when continuing is unsafe; allow `panic!`/`unreachable!` only for proven unreachable states.
- Return type priority: `Result` (caller needs failure reason/recovery) → `Option` (absence is normal) → `bool` (pure yes/no; use an enum if states may grow).

## Async / Tasks
- Encapsulate background tasks in a struct owning `JoinHandle`; spawn via `run() -> Self`.
- For command-driven tasks, store `mpsc::Sender` alongside `JoinHandle` (actor pattern).
- Place task files in the directory matching their responsibility, not necessarily their spawn site (e.g. `crates/moqt/src/modules/moqt/runtime/tasks/uni_stream_receive_task.rs`).

## Async Primitives
- Prefer `tokio` over `std` for async synchronization, file I/O, and time.
- `std::sync::Mutex` is acceptable only when never held across `.await`.

## Imports
- Use `use` declarations; avoid inline full paths unless needed for disambiguation.
- When `std` and `tokio` share a type name in one file, qualify with the module path (e.g. `std::sync::Mutex`) instead of aliasing.

## Module Structure
- Split modules and structs by SOLID principles and functional cohesion.
- Use `foo.rs` + `foo/` instead of `foo/mod.rs`.

## Visibility
- `pub` — exported outside the crate.
- `pub(crate)` — shared within the crate.
- `pub(super)` — parent module only.
- Private — everything else.

## Dependencies
- Minimize external crates; prefer existing dependencies.
- Adding a crate requires an ADR at `docs/architecture/<package_name>/<crate>.md` (e.g. `docs/architecture/gst-plugin-moqt/gstreamer.md`) covering:
  1. **What** — crate and purpose.
  2. **Context** — motivating problem or requirement.
  3. **Alternatives** — other crates/approaches and trade-offs.
  4. **Decision** — final choice and rationale.
- Style reference: `docs/architecture/example/000_jwt_authentication.md`.

## Unsafe
- Prohibited unless required by the compiler or FFI.
