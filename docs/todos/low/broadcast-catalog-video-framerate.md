# [S] List each video track's own frame rate in the MSF catalog

Next: pass the frame rate from `MediaBroadcast.addSource` and `addRendition` (`crates/wasm/src/media_broadcast.rs:116`, `:213`) into the video track info that `build_catalog` reads (`crates/broadcast/src/broadcast.rs:320`).

## Goal

A `loc` video track's `framerate` is the frame rate it was published with: a rendition with `framerate: 15` is listed at 15, not the fixed 30 every track gets today.
A `broadcast` unit test of `build_catalog` with two frame rates shows it.

## Plan

Without a set `framerate` the browser encodes every captured frame, so leave `framerate` out of the catalog rather than guess. No player reads the field yet.
