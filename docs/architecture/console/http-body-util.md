# `http-body-util` for `console` Response Bodies

## Status
Accepted (2026-10-20)

## What
Direct dependency of `console` providing `Full` for complete JSON HTTP responses.

## Context
`hyper` 1.x exposes only the `http_body::Body` trait without concrete body types; `console` returns complete JSON bodies on every route.

## Alternatives
- **Custom `http_body::Body` wrapper**: Reimplements `Full`.

## Decision
Use `http-body-util` (`hyper`'s companion crate), already in the workspace lock.
