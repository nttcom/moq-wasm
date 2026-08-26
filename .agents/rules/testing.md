# Testing Rules

Shared by all agents (Claude Code, Codex).

- Write unit tests inside the target module using `#[cfg(test)] mod tests`.
- Structure every test using the Arrange / Act / Assert pattern.
- Extract Arrange-phase setup into shared utility files; do not duplicate setup across test files.
- Label the phases with `// Arrange`, `// Act`, and `// Assert`. They are an explicit exception to the comment rule in `coding-style.md`: they are allowed even though they only name the phase.
- Append an explanation to a label only when it cannot be derived from the test name, the helper names, or the assertions — never restate them. Prefer renaming the test or extracting a named helper so the bare label suffices; when an explanation is still needed, put it on the label instead of a separate comment line: `// Assert: only session 1 state is removed; other publishers in the namespace survive`.
- `// Act / Assert` is acceptable as one label when both are a single expression.
