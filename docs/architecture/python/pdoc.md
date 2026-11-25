# `pdoc` for the Python SDK API Reference (`python`)

**Status:** Accepted (2026-11-18)

## What

Add `pdoc` to the `dev` dependency group of `sdk/python`. `moq_wasm/__init__.py` embeds the README as its module docstring, and `npm run docs` in `examples/browser` runs `uv run pdoc` over `moq_wasm`, `moq_wasm.media`, `moq_wasm.codecs`, and `moq_wasm.pipecat`, outputting HTML to `examples/browser/public/docs/python`.

## Context

The Python SDK is documented via its README and docstrings, and GitHub Pages should publish both alongside the browser SDK reference.

## Alternatives

- **Sphinx with `autodoc`:** Can mock `_native` instead of compiling it, but requires configuration files, a theme, and reStructuredText or a Markdown plugin for the README.

## Decision

Use `pdoc` to generate static HTML directly from existing docstrings and type annotations with zero configuration.

## Consequences

- **Positive:** No configuration files; reads docstrings and type annotations as-is.
- **Negative:** `pdoc` imports the package, forcing GitHub Pages deploys to compile `moq_wasm._native` (adding several minutes).
- **Negative:** Methods defined on `_native` classes render without signatures or docstrings.
