# [S] Treat a draft-18 control stream that ends with its session as a close, not a protocol violation

Next: find which arm `crates/moqt/src/modules/moqt/version/draft18/request_streams.rs:225` takes when a browser closes its WebTransport session (`Ok(None)` or `Closed(_)` other than `ConnectionLost`), and whether the session's close is known by then.

## Goal

A browser page that closes (`tests/remote-broadcast-e2e.spec.ts`, "a page that closes withdraws its broadcast at once", via `node tests/browser-e2e/run-media-e2e.mjs`) logs no ERROR on the relay.
Today each such session logs `Protocol violation on a draft-18 stream; closing session reason="the control stream closed"`, sometimes `failed to send protocol violation event error=SendError` (`crates/moqt/src/modules/moqt/domains/session_context.rs:581`), and the relay's `session closed for a protocol violation`.

## Plan

draft-ietf-moq-transport-18 §3.3 forbids closing the control stream at the transport layer, so a peer that closes only the stream stays a violation; the fix is to tell that apart from the session closing.
