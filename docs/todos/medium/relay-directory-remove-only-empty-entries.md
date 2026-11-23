# [S] Remove a directory namespace entry only while it is still empty

Next: in `unregister_peer` (`services/relay/src/modules/directory.rs:213`), replace `namespaces.remove(namespace)` with `namespaces.remove_if(namespace, |_, peers| peers.is_empty())`.

## Goal

A publication or namespace subscription registered while another session's last one is removed stays in the directory.
Now `unregister_peer` drops its guard on the entry, then removes it unconditionally: a `register_publish_namespace` from another session in between inserts into the emptied map and is removed with it.

## Plan

The same check-then-remove is in `purge_relay_publish_namespaces` (`:495`), `unregister_subscribe_namespace` (`:592`) and `remove_namespace_subscriptions` (`:617`).
Found by reading the code; not reproduced.
