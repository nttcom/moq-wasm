# [S] Check a migrated subscription by the rules of the connection it moved to

Next: when a subscription is carried to the new connection (`crates/moqt/src/modules/moqt/domains/session/migration_task.rs:697`), update `DataChannel.version`, which is set from the first connection only (`domains/session_context.rs:515`).

## Goal

A subscription that GOAWAY moved from a draft-14 server to a draft-18 server accepts what draft-18 allows.
Reproduced: a datagram after a subgroup stream is refused as `MixedForwardingPreference` (`data_plane/data_receiver.rs:67`), a draft-14 rule.
The datagram checks at `:78` read the same version; the draft-18 → draft-14 direction was not run.
