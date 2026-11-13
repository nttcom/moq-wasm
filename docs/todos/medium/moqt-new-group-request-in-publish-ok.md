# [S] Act on a NEW_GROUP_REQUEST in a draft-18 PUBLISH_OK

Next: carry `Parameter::NewGroupRequest` out of `into_publish_ok` (`crates/moqt/src/modules/moqt/version/draft18/messages.rs:1047`), which reads it and drops it, to where the `new_group_request` of a SUBSCRIBE goes.

## Goal

A publisher whose PUBLISH is answered with NEW_GROUP_REQUEST starts a new group, as it does for one in SUBSCRIBE or REQUEST_UPDATE (draft-18 §10.2.13: the parameter "MAY appear in PUBLISH_OK, SUBSCRIBE or REQUEST_UPDATE").
Read from the code, not run: `PublishOk` has no field for it.
