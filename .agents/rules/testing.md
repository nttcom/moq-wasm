# Testing Rules

Shared by all agents.

- Colocate unit tests in `#[cfg(test)] mod tests`.
- Structure every test with Arrange / Act / Assert, labeled `// Arrange`, `// Act`, and `// Assert` (`// Act / Assert` is fine for a single expression).
- Put phase explanations on the label line (`// Assert: only session 1 state is removed; other publishers survive`), not on a separate comment line.
- Share Arrange setup in test utility files; do not duplicate setup across test files.

## Property-Based Tests
- Add `proptest` properties in the same PR when adding or reworking:
  - chunked parsers, codecs, muxers, or demuxers;
  - state machines;
  - time or ID arithmetic that rounds, wraps, or can overflow;
  - logic checkable against a simple model (e.g. brute-force search or set).
- Specify the module's invariants (round-trip, chunk-invariance, model equivalence, monotonicity)—not a single bug repro.
- Bias generators toward real boundaries (encoded length edges, wrap points, `0`/max, repeats, backward steps); never hard-code toward a known bug.
- Before committing, temporarily break the tested code (flip a comparison, drop a rounding step) and confirm the property fails.
- Bug fixes found by reading code get an example test; add a property only when a general invariant fits the module.
