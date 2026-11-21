# [M] End a media source whose track waits for an unanswered PUBLISH

Next: bound or cancel the PUBLISH a track's first send awaits (`Origin::publish`, `crates/moqt/src/modules/moqt/domains/origin.rs`) when its media source ends, and find why the relay leaves it unanswered.

## Goal

A namespace withdrawn while a source's own PUBLISH_NAMESPACE and PUBLISHes were under way left the PUBLISH unanswered on a draft-18 relay; closing the Python SDK's media broadcast, queued behind that source's publish task, then hung until the session closed (Python SDK review, R5-6).
Ending a media source (`MediaSource::end`) returns within a bounded time whatever the relay answers.

## Plan

Aborting a task whose PUBLISH is still unanswered makes moqt send PUBLISH_DONE (InternalError) for it; check that path when bounding the wait.
