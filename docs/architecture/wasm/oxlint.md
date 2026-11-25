# `oxlint` for Unawaited Promises in the Browser SDK (`wasm`)

**Status:** Accepted (2026-11-13)

## What

Add `oxlint` and `oxlint-tsgolint` as `devDependencies` of `moq-wasm` and run `oxlint --type-aware src` (`npm run lint:promises`) in the `sdk-package` CI job. `.oxlintrc.json` disables all default categories and enables only `typescript/no-floating-promises`.

## Context

A Promise that is neither `await`ed nor explicitly ignored with `void` drops rejections and lets subsequent code run prematurely. `tsc` does not catch floating promises, and checking them requires type information; `sdk/js/js` previously had no linter in CI.

## Alternatives

- **`typescript-eslint`:** Requires the `typescript` `<6.1` compiler API (removed in TypeScript 7), forcing ESLint, `typescript-eslint`, and a second older `typescript` installation alongside the workspace compiler.

## Decision

Use `oxlint` with `oxlint-tsgolint`, scoped solely to `typescript/no-floating-promises`.

## Consequences

- **Positive:** `oxlint-tsgolint` type-checks with the same TypeScript 7 compiler as `tsc` and adds only two packages (plus platform binaries)—no ESLint stack.
- **Negative:** Type-aware linting in `oxlint` and the TypeScript 7 API are less mature than `typescript-eslint`.
- **Neutral:** Reads `pkg/moqt.d.ts`, so it runs after the wasm build in the existing `sdk-package` job.
