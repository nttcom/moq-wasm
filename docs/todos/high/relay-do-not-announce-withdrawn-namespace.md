# [S] Do not leave a withdrawn namespace announced to a new namespace subscriber

Next: in `services/relay/src/modules/control_plane/sequences/subscribe_namespace.rs:325`, the namespaces to announce are a snapshot and each PUBLISH_NAMESPACE is awaited (`:357`); record a namespace as announced before the await, or check it is still published after it.

## Goal

A client that subscribes to a prefix while a publisher under it leaves ends up without that publisher's namespace.
Reproduced: a namespace withdrawn after the snapshot has its PUBLISH_NAMESPACE_DONE dropped as "not announced on the session" (`control_plane/control_message_forwarder.rs:99`), then the loop sends its PUBLISH_NAMESPACE.
The client keeps seeing a participant who left until it subscribes again.
