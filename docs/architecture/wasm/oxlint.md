# ADR: oxlint for unawaited promises in the browser SDK

## Status
Accepted

## Date
2026-11-13

## Context
A promise the SDK neither awaits nor marks with `void` loses its rejection and lets the next step run before it
settles. `tsc` does not report it, and the browser SDK (`crates/wasm/js`) had no linter, so nothing in CI caught it.
The check needs type information: whether an expression is a promise depends on its type.

## Decision
Add **`oxlint`** and **`oxlint-tsgolint`** as devDependencies of `moq-wasm` and run
`oxlint --type-aware src` (`npm run lint:promises`) in the `sdk-package` CI job. `.oxlintrc.json` turns off every
default category and enables one rule, `typescript/no-floating-promises`, a port of the typescript-eslint rule
of the same name.

## Consequences

### Positive
- `oxlint-tsgolint` type checks with the TypeScript 7 compiler the workspace already uses, so the rule sees the
  same types as `tsc`.
- No ESLint, parser or plugin packages; the lock file gains only the two packages and their per-platform binaries.

### Negative
- Type-aware linting in oxlint and the TypeScript 7 API it builds on are newer than typescript-eslint.

### Neutral
- The check reads `pkg/moqt.d.ts`, so it runs after the wasm build, in the job that already does it.

## Alternatives Considered

### typescript-eslint
The rule's original. It needs the TypeScript compiler API of `typescript` `<6.1`, which TypeScript 7 no longer
ships, so it would need ESLint, typescript-eslint and a second, older `typescript` installed beside the one the
workspace builds with.
