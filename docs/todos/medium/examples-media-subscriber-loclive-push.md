# [XS] Pass the codec to LocLive.push in the media subscriber

Next: add the track's codec to `locLive.push` at `examples/browser/examples/media/subscriber/main.ts:178`.

## Goal

`npm --prefix examples/browser run lint` passes; it now fails with TS2554 there, since `LocLive.push` takes a `codec` (`crates/wasm/js/src/media/player/locLive.ts:167`).
