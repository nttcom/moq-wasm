# [S] State why a review FETCH stops at the newest closed group

Next: read `resolve_fetch_range` in `docs/architecture/relay/architecture.md` against the comment at `crates/wasm/js/src/media/player/rewind.ts:78`.

## Goal

The comments on `newestClosed` and `resolveSeekTarget` (`rewind.ts:78`, `rewind.ts:99`) give the reason a review avoids the open live-edge group. They say the FETCH "leaves the relay cache and is forwarded to a publisher that does not serve FETCH". The relay now serves an open group from cache while it has an open subgroup, and `crates/broadcast` publishers answer forwarded FETCHes from a 60 s cache.

## Plan

Find the actual reason (perhaps the FETCH waits for the group to close), then reword both comments and the invariant at `docs/architecture/browser-examples/live-player.md:233`.
