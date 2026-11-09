# [XS] Reject refused WebTransport requests off the accept loop

Next: move the `request.reject(...)` await in `WtConnectionCreator::accept_new_transport` (`crates/moqt/src/modules/transport/webtransport/wt_connection_creator.rs:163`) into a spawned task.

## Goal

After `refuse_new_connections` (endpoint shutdown), a peer that gives zero stream credit cannot stall the WebTransport-only accept loop. Today the loop awaits each reject before it accepts the next request.
