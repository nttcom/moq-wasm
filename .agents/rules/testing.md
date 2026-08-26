# Testing Rules

Shared testing rules for all agents (Claude Code, Codex). Read this before writing or modifying test code.

- Write unit tests inside the target module using `#[cfg(test)] mod tests`.
- Structure test code using the Arrange / Act / Assert pattern.
- Extract Arrange-phase setup into shared utility files; do not duplicate setup across test files.
- In the Act and Assert phases, add comments at an appropriate granularity so the test intent and verification points are easy to scan.
- If the flow of operations or the expected result is not immediately obvious, do not leave the Act and Assert phases without comments.
- When modifying tests, confirm that shared Arrange utilities still have reusable responsibilities and scope, and that the comments in Act and Assert remain sufficiently descriptive.
- After making changes, run `cargo test -p <package_name>` for the affected package to verify no regressions.
