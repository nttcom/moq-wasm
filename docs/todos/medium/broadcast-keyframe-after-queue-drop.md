# [S] Ask the source for a keyframe when the publish queue drops video

Next: in `crates/broadcast/src/publish_queue.rs:163` `record_drop`, notify `SourceTracks::keyframe_requests` when it is set.

## Goal

A publisher whose queue overflowed resumes video for every viewer within a frame of the queue draining. Today it drops media "until a keyframe fits", which the browser's default interval makes up to 30 s. Not reproduced.

## Plan

`PublishQueue::open` does not hold the `SourceTracks`; pass the `Notify` in from `BroadcastWriter::run`.
