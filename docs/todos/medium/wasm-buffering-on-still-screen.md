# [S] Do not show buffering while a shared screen stands still

Next: decide in `crates/wasm/js/src/media/player/stallWatch.ts:17` how a still source differs from a held-up one before its next frame arrives.

## Goal

`PlayerControls` hides `buffering` (`crates/wasm/js/src/media/controls/playerControls.ts:50`) while a publisher sends nothing because its screen does not change; it still shows when objects are held up on the path.

## Plan

The automatic quality already ignores stalls and reads the lag of the next frame instead; the indicator cannot wait for that frame, so it needs a signal from the publisher or a looser rule for sources such as `screen`.
