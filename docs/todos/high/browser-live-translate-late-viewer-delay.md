# [M] Play a late live-translate viewer at the 5 s delay again

Next: run `node tests/browser-e2e/run-live-translate-e2e.mjs` on the SDK commits since 2026-11-09, when the suite last passed, to find the one that broke the second test.

## Goal

`a viewer who joins after the translation started hears the original audio` (`examples/browser/tests/live-translate-e2e.spec.ts:102`) passes: the late viewer's `viewerDelayMs` is under 6 000.
On 2026-11-12 it was about 10 000 in three runs on master; the first test of the file passed each time.

## Plan

CI does not run this suite (`docs/todos/low/ci-live-translate-browser-e2e.md`), which is why the regression was merged.
