# TypeDoc for the Browser SDK API Reference (`wasm`)

**Status:** Accepted (2026-11-18)

## What

Add `typedoc` in `sdk/js/js/typedoc/` as a standalone package with its own lock file outside the npm workspace. Its `typedoc.json` ingests the README and four entry points (`moq-wasm`, `moq-wasm/media`, `moq-wasm/recording`, `moq-wasm/element`); `npm run docs` in `examples/browser` writes static HTML to `examples/browser/public/docs/js` for Vite and GitHub Pages.

## Context

`sdk/js/js` is documented via its README and TSDoc comments, previously visible only in source or `.d.ts` files. GitHub Pages should publish both as a browsable guide and API reference.

## Alternatives

- **`npx typedoc` in the deploy job:** Avoids committing a lock file (and its advisory), but lets TypeDoc dependencies float unpinned on every deploy.
- **`api-documenter`:** Consumes `api-extractor`'s `.api.json`, but outputs raw Markdown that still requires a separate site renderer.

## Decision

Run pinned `typedoc` from an isolated `sdk/js/js/typedoc/` package.

## Consequences

- **Positive:** One command builds a searchable site directly from the README and TSDoc with zero duplicated prose.
- **Negative:** TypeDoc supports `typescript` up to 6.0 (not the workspace's TypeScript 7) as a peer dependency. Placing it inside the workspace caused npm to install TypeScript 6 in `sdk/js/js/node_modules/.bin`, shadowing workspace `tsc` and breaking `api-extractor` diffs—hence the isolated package and lock file.
- **Negative:** `skipErrorChecking` is required because sources use APIs missing from TypeScript 6's lib definitions (such as `Uint8Array.fromBase64`).
- **Negative:** TypeDoc depends on `minimatch` 10 (affected by `GHSA-g3hv-75wg-m249`) and calls `Minimatch.matchOne`, which `minimatch` 11 removed, blocking the workspace's override. Risk is limited because TypeDoc only evaluates its own config globs at build time.
