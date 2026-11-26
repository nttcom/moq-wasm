# [S] Find why the live-translate E2E streamer of a temporary app sometimes never connects

Next: open the Playwright trace a failed `live_translate_e2e` job uploads and follow the streamer page after the click at `examples/browser/tests/live-translate-e2e.spec.ts:28`.

## Goal

`a broadcast of a temporary app is translated once the streamer hands its token over` passes on every run.
It failed twice on 2026-11-26 on unrelated pull requests, at `live-translate-e2e.spec.ts:30`: `streamer-viewer-link` had no `href` after 15 s, while the same job passed on other pull requests started at the same time.
In a failed run `temporary-apps` logs `temporary token issued`, and then the relay logs no session for the streamer until the timeout.
