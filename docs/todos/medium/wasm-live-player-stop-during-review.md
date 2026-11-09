# [M] Make LivePlayer.stop return while review FETCHes are in flight

Next: find which await in `forgetPublisher` never resolves, starting at `crates/wasm/js/src/media/player/livePlayer.ts:385` (`retireAll`) and `:387` (`textTracks.unsubscribeAll`).

## Goal

`live viewer rewinds to groups from before it started watching` (`examples/browser/tests/live-viewer-e2e.spec.ts:318`) passes every run. In CI `live_viewer_e2e (live-srt)` it failed in cleanup with `Expected: "Not connected"`: after Stop, the relay logged both UNSUBSCRIBEs, then `player.stop()` did not return for 16 s, so `stopStream` (`examples/browser/examples/live-viewer/main.ts:113`) never reached `session.close()`.
Several review FETCHes (relay request_id 72 to 86, 241 objects) were open when Stop was pressed.
