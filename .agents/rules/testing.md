# Testing Rules

Shared by all agents (Claude Code, Codex).

- Write unit tests inside the target module using `#[cfg(test)] mod tests`.
- Structure every test using the Arrange / Act / Assert pattern.
- Extract Arrange-phase setup into shared utility files; do not duplicate setup across test files.
- Label the phases with `// Arrange`, `// Act`, and `// Assert`. These are exempt from the minimal-comment rule in `coding-style.md` and are allowed even when they only name the phase.
- When the flow or the expected result is not obvious, append the explanation to the label instead of adding a separate comment line: `// Assert: only session 1 state is removed; other publishers in the namespace survive`.
- `// Act / Assert` is acceptable as one label when both are a single expression.
