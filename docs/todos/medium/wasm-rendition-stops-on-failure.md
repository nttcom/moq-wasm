# [S] Stop a browser rendition once its track refuses a frame

Next: in `crates/wasm/src/media_broadcast.rs` (`add_rendition`'s task, the `rendition push failed` warning), end the task on the first failed push, as `crates/python/src/media_broadcast/rendition_task.rs` does.

## Goal

After the session ends, `pushRenditionVideo` rejects as `pushVideo` does, instead of succeeding while the task logs `rendition push failed` for every frame.
