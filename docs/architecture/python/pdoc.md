# ADR: pdoc for the Python SDK's API reference

## Status
Accepted

## Date
2026-11-18

## Context
The Python SDK (`crates/python`) is documented by its README and its docstrings. The Example site on GitHub Pages
should serve both as pages, next to the browser SDK's reference.

## Decision
Add **`pdoc`** to the `dev` dependency group. `moq_wasm/__init__.py` includes the README as its docstring, and
`npm run docs` in `examples/browser` runs `uv run pdoc` on `moq_wasm`, `moq_wasm.media`, `moq_wasm.codecs` and
`moq_wasm.pipecat`, writing the HTML to `examples/browser/public/docs/python`.

## Consequences

### Positive
- No configuration file: pdoc reads the docstrings and type annotations as they are.

### Negative
- pdoc imports the package, so the docs build compiles the native module `moq_wasm._native`, which adds minutes to
  the GitHub Pages deploy.
- The methods of the classes `_native` defines show neither a signature nor a docstring.

## Alternatives Considered

### Sphinx with autodoc
It can mock `_native` instead of building it, but needs a configuration, a theme and reStructuredText or a
Markdown extension around the README.
