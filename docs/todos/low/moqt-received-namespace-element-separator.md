# [M] Keep a received namespace element that contains `/` one element

Next: trace the received Track Namespace tuples the control-plane handlers join with `/` (e.g. `crates/moqt/src/modules/moqt/control_plane/handler/track_status_handler.rs:54`) back through `namespace_tuple` in `crates/moqt/src/modules/moqt/domains.rs:28`.

## Goal

A third-party peer that sends PUBLISH_NAMESPACE `["anon", "directory/filename"]` is seen by this crate and the relay with two elements; today the `/`-joined keys split it into three.

## Plan

`moqt::client::Session` rejects such elements on the sending side (`RequestFailure::InvalidNamespace`); only peers outside this crate can send them.
