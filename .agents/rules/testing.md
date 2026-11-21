# Testing Rules

Shared by all agents.

- Write unit tests inside the target module using `#[cfg(test)] mod tests`.
- Structure every test using the Arrange / Act / Assert pattern.
- Extract Arrange-phase setup into shared utility files; do not duplicate setup across test files.
- Label the phases with `// Arrange`, `// Act`, and `// Assert`.
- Put an explanation of a phase on its label instead of a separate comment line: `// Assert: only session 1 state is removed; other publishers in the namespace survive`.
- `// Act / Assert` is acceptable as one label when both are a single expression.

## Property-based tests
- When a change adds or reworks one of the following, state what it guarantees as `proptest` properties in the same pull request:
  - a parser fed in chunks, a codec, a muxer or a demuxer;
  - a state machine;
  - arithmetic on times or IDs that rounds, wraps or can overflow;
  - logic a simple model can check, such as a brute-force search or a set.
- Write the property as the module's specification, not as the reproduction of one bug: a round trip gives back its input, any chunking gives the same result, the result equals the model's, a sequence stays monotonic.
- Make the generators favour the boundaries real input reaches: the edges of each encoded length, values near a wrap, 0 and the maximum, repeats and steps back. Uniform values almost never reach them. Do not bend a generator towards a bug you already know.
- Before committing, break the code the property checks (flip a comparison, drop a rounding step) and confirm the property fails. A property that never reaches that code passes either way.
- A fix for a bug found by reading the code gets an example test; add a property only when a general one fits the module.
