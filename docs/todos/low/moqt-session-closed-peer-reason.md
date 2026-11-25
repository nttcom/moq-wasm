# [S] Carry the peer's close code and reason in `RequestFailure::SessionClosed`

Next: give `SessionClosed` (`crates/moqt/src/modules/moqt/domains/request_failure.rs:20`) the close code and reason the transport already logs (`crates/moqt/src/modules/transport/quic/quic_connection.rs`).

## Goal

A relay that closes a session at token expiry sends `authorization token expired (code 24)`, yet the publisher's error reads only `the session closed before the response arrived`.
`obs-moqt` shows that text in OBS, so the user cannot tell an expired token from a lost relay; the same holds for every `broadcast` publisher.
