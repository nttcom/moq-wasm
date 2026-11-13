# [XS] Forget the announced namespaces of the prefix a namespace subscription moves away from

Next: in the prefix update (`services/relay/src/modules/control_plane/sequences/subscribe_namespace.rs:244`), call `forget_announced_namespaces` for the released prefix, as `unsubscribe_namespace.rs:36` does.

## Goal

A namespace is announced again after its subscriber's prefix moved away and back.
Reproduced at the relay (prefix `room` → `lobby` → `room`): the second announcement is skipped as already announced (`control_plane/control_message_forwarder.rs:52`), while moqt forgot the namespace when the prefix left it out (`crates/moqt/src/modules/moqt/version/draft18.rs:470`, read, not run).
No client in this repository sends a prefix update yet.
