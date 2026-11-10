# [S] Cache the dependencies of every compose_e2e suite

Next: give the `Swatinem/rust-cache` step of `compose_e2e` at `.github/workflows/e2e.yml:1104` a `key` that separates the suites whose dependencies differ.

## Goal

No `compose_e2e` row compiles a third-party crate when its cache is restored. Today the eight rows save under one key, so one row's cache serves all: `live-translate`, `moqtsink`, `auth` and `cascading-relay` compile 78, 56, 42 and 26 third-party crates on every run, `aws-lc-sys` among them.

## Plan

`fetch`, `multiple-publishers`, `object-dedup` and `cache-eviction` compile none, so they can keep one key. Open question: a cache per remaining suite adds to the 8.2 GB one generation of Rust caches takes of the 10 GB limit; measure the sizes first.
