# [S] Make the compose_e2e rows use the shared e2e Rust cache

Next: find why `compose_e2e (fetch)` recompiles `proc-macro2` after restoring `v0-rust-e2e-Linux-x64-966934f9-e8ac0105` with a full match (run 45559814943, job 139021599090): compare `target/debug/.fingerprint` of `proc-macro2` in the `rust_cache` job (`.github/workflows/e2e.yml:402`) and in that row.

## Goal

No `compose_e2e` row compiles a third-party crate when its cache is restored. On master after #2033 every row still does: fetch 137, cascading-relay 145, auth 167, moqtsink 169, live-translate 180, `aws-lc-sys` among them.

## Plan

The `rust_cache` job built every suite's crate (757 `Compiling` lines) before saving the 673 MB cache, so the artifacts were built; check what rust-cache's target cleaning keeps, and whether a row's cargo invocation differs (features, env such as `CI`) from the one in `rust_cache`.
The `rust_cache` job skips its builds on a cache hit, so a cache saved incomplete for a key stays incomplete until `Cargo.lock` changes.
