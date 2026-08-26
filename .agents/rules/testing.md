# Testing Rules

Shared testing rules for all agents (Claude Code, Codex). Read this before writing or modifying test code.

## Placement and Structure
- Write unit tests inside the target module using `#[cfg(test)] mod tests`.
- Structure every test using the Arrange / Act / Assert pattern.
- Extract Arrange-phase setup into shared utility files; do not duplicate setup across test files.

## Phase Comments
- The "keep comments to a minimum" rule in `coding-style.md` does not apply to phase separators: `// Arrange`, `// Act`, and `// Assert` comments are always allowed, even when they only label the phase.
- In the Act and Assert phases, add comments at an appropriate granularity so the test intent and verification points are easy to scan.
- If the flow of operations or the expected result is not immediately obvious, do not leave the Act and Assert phases without comments.
- When a phase needs explanation, append it to the label instead of adding a separate comment line:

  ```rust
  // Arrange: register the namespace and track alias for the session
  // Act: remove all state associated with session 1
  // Assert: only session 1 state is removed; other publishers in the namespace survive
  ```

- When Act and Assert are a single expression, `// Act / Assert` is acceptable as one label.

## Maintenance
- When modifying tests, confirm that shared Arrange utilities still have reusable responsibilities and scope, and that the comments in Act and Assert remain sufficiently descriptive.
- After making changes, run `cargo test -p <package_name>` for the affected package to verify no regressions.
