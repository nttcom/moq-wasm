# [XS] Drop the broadcast checks that never change an outcome

Next: in `crates/broadcast/src/relay_connection.rs` (`serve_subscribe`), drop `RelayState::publishes`; a namespace without a catalog entry has no subscribable or published track, so `!(subscribable || published)` already returns the handler.

## Goal

cargo-mutants finds no surviving mutant in `RelayState::publishes` or `TrackPublisher::announce` (`crates/broadcast/src/track_publisher.rs`); each sample follows its config, whose catalog update already announces the namespace.
