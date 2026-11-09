# [S] Delete a room lock only if it is still the one that was read

Next: read `Store::unlock_room` in `services/recorder/src/store.rs:176`.

## Goal

Unlocking never deletes a lock another recording took in between. Today it reads the holder and then calls `delete`; a request that takes the room between the two loses its lock, and a third request can start a second recording of the room.

## Plan

`object_store` 0.14.2 has no conditional delete (`ObjectStore::delete` takes only a path); it offers conditional puts (`PutMode::Create`, `PutMode::Update`). Cloud Storage's JSON API deletes with `ifGenerationMatch`, which `crates/app-registry/src/cloud_storage.rs` already reads generations for. Open: call that API directly for the delete, or replace delete with a conditional overwrite to an "unlocked" marker.
