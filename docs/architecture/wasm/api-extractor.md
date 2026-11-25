# API Extractor Reports for the Browser SDK (`wasm`)

**Status:** Accepted (2026-11-13)

## What

Add `@microsoft/api-extractor` as a `devDependency` of `moq-wasm`. For each typed entry point (`.`, `./media`, `./recording`, `./element`), it reads `types/<entry>/index.d.ts` and writes a committed API report at `sdk/js/js/api/<entry>.api.md`.

- `npm run api:check` (run in the `sdk-package` CI job) fails if built declarations differ from the committed reports.
- `npm run api:update` regenerates the reports.
- Release tags and TSDoc syntax are unchecked (the SDK uses `///` comments and no release tags); reports cover only exported declarations and referenced unexported types (`ae-forgotten-export`).

## Context

`tsc` writes declaration files into uncommitted `types/`, so changes to `moq-wasm`'s public surface (renamed options, widened return types) were invisible in PR diffs.

## Alternatives

- **Commit concatenated `types/**/*.d.ts`:** Zero new dependencies, but includes internal modules so internal refactors churn the diff.
- **`dts-bundle-generator` / `rollup-plugin-dts`:** Bundle declarations for publishing rather than generating reviewable API reports, and neither is in the workspace.

## Decision

Adopt `@microsoft/api-extractor` with one config and report per entry point.

## Consequences

- **Positive:** Any public API change appears directly as a diff in `sdk/js/js/api/*.api.md`; internal refactors stay silent.
- **Negative:** Four separate configs and reports (one per entry point).
- **Negative:** Uses its own TypeScript 6.x analyzer, which cannot emit `typeof import('...')` (the SDK uses `typeof` a namespace import instead).
- **Negative:** Referenced unexported types are reported by name only, so internal changes to them do not appear in diffs.
- **Neutral:** Reports are excluded from Prettier (`sdk/js/js/.prettierignore`) because `api:check` compares byte-for-byte.
