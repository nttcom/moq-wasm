# [S] Read the browser WebTransport stats from `WebTransport.getStats()`

Next: check whether Chrome implements `WebTransport.getStats()`; if so, call it in `BrowserConnection::stats` (`crates/moqt/src/modules/transport/browser/browser_connection.rs`).

## Goal

`MoqSession.transportStats()` over WebTransport reports the browser's own figures, with RTT, loss and packet counts, instead of the bytes `BrowserStats` counts.

## Plan

`stats()` is synchronous and `getStats()` returns a promise: cache the last result, or make `stats()` async.
Then delete `browser_stats.rs` and update the README's `transportStats()` notes and the CHANGELOG.
