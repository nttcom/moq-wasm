# `static-files` Architecture

## Status

Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope

`static-files` serves the built files of a web page from a directory for a
service that ships its page in its image: `console`.
`StaticFiles::get` maps a URI path to a file under the root and reads it whole.

## Invariants

- A path is served only when every segment is a plain file name (no empty or
  dot-leading segment, only alphanumerics, `.`, `_` and `-`), so no path reaches
  outside the root; `/` is `index.html`.
- Only the file types a page is made of are served (HTML, JS, CSS, SVG, PNG,
  ICO, WOFF2, WASM); a file of any other type under the root is not reachable.
- A file under `assets/` carries a content hash in its name, so it is marked
  `immutable` for the caller's cache headers.
