# [M] Skip E2E when Cargo.lock changes only an unrelated crate's version

Next: in the `changes` job of `.github/workflows/e2e.yml`, replace the `Cargo.lock` pattern in each suite's case with a check of which packages the lock diff touches.

## Goal

A release PR that bumps only `obs-moqt` (as #2096 did) runs no E2E suite. Today every suite runs, because each one matches any `Cargo.lock` change.

## Plan

The same pattern selects jobs in `python.yml`, `deploy.yml` and the `sdk-package` lint job; one helper that maps a lock diff to the changed package names could serve all of them.
