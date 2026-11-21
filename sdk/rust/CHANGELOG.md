# Changelog

## Unreleased

- The first release of `moq-wasm` for Rust, as a git dependency on the
  `rust-sdk-v0.0.1` tag, tested on Linux x86_64, Linux aarch64 and macOS
  arm64. The session offers draft-18, then draft-14.

- `moq_wasm::connect` opens a `Session`, which dereferences to
  `moqt::client::Session` for publishing, subscribing, fetching and watching
  namespaces. It follows GOAWAY and replaces a lost connection for up to
  60 s; `ConnectOptions::refresh_token` keeps its token fresh, and
  `closed()` says how it ended (`SessionEnd`).

- `moq_wasm::media::create_broadcast` publishes a broadcast: tracks and MSF
  event timelines registered before its namespace, and media sources of
  encoded H.264, H.265, VP8, VP9 or AV1 video (with renditions) and AAC or
  Opus audio, with the catalog the browser and Python SDKs read.

- `moq_wasm::media::watch_broadcasts` runs a handler per broadcast under a
  prefix, skipping the session's own; a `RemoteBroadcast` reads its
  catalogs, follows a track and reads an event timeline, and
  `Catalog::select` picks a track.
