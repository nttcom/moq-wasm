# [XS] Reject addSource when the broadcast closes while its tracks are declared

Next: in `publish` (`crates/wasm/js/src/media/localBroadcast.ts:193`), check `closed` again after `await declareTracks` and release the captured tracks before throwing.

## Goal

An `addSource` pending when `close()` is called rejects and leaves no camera on.
Reproduced with fakes in Node: it resolves with a live source after `close()` resolved; the camera stays live, an encoder worker runs and the source is listed and published on the closed broadcast until the session closes.
