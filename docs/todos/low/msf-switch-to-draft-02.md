# [L] Move the catalogs and timelines to draft-ietf-moq-msf-02 once it is published

Next: when -02 is on the Datatracker, add it under `docs/spec/` and compare it with the editor's copy of 2026-11-02 (moq-wg/msf `main`), which this TODO is written against.

## Goal

Writers follow -02: `version` `"draft-02"`; event timeline index keys `T`/`L`/`M` (§8.2: `examples/browser/examples/meeting/src/session/localSession.ts:356`, the transcriber, the Python bots); no `depends` on the wallclock-indexed chat (`crates/broadcast/src/broadcast.rs:392`) and transcript (`services/recorder/src/vod.rs:241`, §8.3); a media timeline names a whole renderGroup by its number (§7.2, `crates/broadcast/src/broadcast.rs:377`); each `loc` track carries the fields §5.2.4.1 requires for its role (the audio placeholder, `crates/broadcast/src/broadcast.rs:357`, has no `samplerate` or `channelConfig`; the browser's video `framerate` is optional, `crates/wasm/js/src/media/publish.ts:326`).
Readers follow: `mediaTimelineOf` (`crates/wasm/js/src/media/catalog.ts:121`) and the recorder (`services/recorder/src/track_selection.rs:204`) resolve a renderGroup number; the recorder stores `T` records as `M` (`services/recorder/src/recording_clock.rs:71`) and reads `M` (`services/recorder/src/vod.rs:198`).
Unit tests of `broadcast`, `recorder` and the browser SDK with -02 catalogs, and the meeting and recorder E2E, show it.

## Plan

Settled 2026-11-09: switch everything when -02 is published, not before, because -01 §8.2 requires the `depends` and the lowercase keys. If -02 differs from the editor's copy, follow -02.
