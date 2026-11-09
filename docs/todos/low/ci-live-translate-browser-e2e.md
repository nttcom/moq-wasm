# [M] Run the live translation browser E2E in CI

Next: copy the `live_viewer_e2e` job at `.github/workflows/e2e.yml:446` into a `live_translate_e2e` job that runs `node tests/browser-e2e/run-live-translate-e2e.mjs`.

## Goal

`examples/browser/tests/live-translate-e2e.spec.ts` runs on every change to the live translation pages, the SDK player or `services/live-translate`; today CI runs only the Rust suite `tests/live-translate-e2e`, so a broken Streamer or Viewer page is found by hand.

## Plan

The runner builds `live-translate` with cargo, so the job installs GStreamer through `.github/actions/install-ffmpeg-deps` as the `moqtsink` suite does, and the `changes` job selects it on the paths of `examples/browser/examples/live-translate/*`, `crates/wasm/js/*` and `services/live-translate/*`.
