# ADR: TypeDoc for the browser SDK's API reference

## Status
Accepted

## Date
2026-11-18

## Context
The browser SDK (`crates/wasm/js`) is documented by its README and its TSDoc comments, which only someone reading
the source or the `.d.ts` files sees. The Example site on GitHub Pages should serve both as pages: the README as a
guide and every exported class, function and type of the `moq-wasm`, `moq-wasm/media` and `moq-wasm/recording`
entry points as a reference.

## Decision
Add **`typedoc`** as a devDependency of `moq-wasm`. `typedoc.json` takes the three entry points and the README;
`npm run docs` in `examples/browser` writes the HTML to `examples/browser/public/docs/js`, which Vite serves and
copies into the site.

## Consequences

### Positive
- One command turns the README and the TSDoc comments into a searchable site; nothing is written twice.

### Negative
- TypeDoc supports `typescript` up to 6.0, not the TypeScript 7 the workspace builds with, so npm installs a second,
  nested `typescript` 6 for it and `skipErrorChecking` stops it from type checking the sources, which use APIs
  TypeScript 6's libraries lack (`Uint8Array.fromBase64`).
- TypeDoc calls `Minimatch.matchOne`, which minimatch 11 removed, so the root `overrides` gives it minimatch 10,
  which GHSA-g3hv-75wg-m249 (DoS through glob patterns) still affects. TypeDoc only matches the globs of its own
  configuration, at build time.

## Alternatives Considered

### api-documenter
It reads the `.api.json` that `api-extractor` already produces, but writes Markdown, which would need a renderer
and a site around it.
