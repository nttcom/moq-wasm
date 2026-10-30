# Changelog

A `sdk-v<version>` tag releases the section of its version as the body of the
GitHub Release (`.github/workflows/sdk-release.yml`).

## 0.1.0

The first release, as a tarball attached to the GitHub Release.

- `moq-wasm`: `MoqSession` connects to a relay over WebTransport, or WebSocket
  where WebTransport is unavailable, and sends SUBSCRIBE, FETCH, TRACK_STATUS,
  SUBSCRIBE_NAMESPACE and PUBLISH_NAMESPACE (draft-ietf-moq-transport-14).
  `publishTrack` returns a `TrackWriter` that numbers, caches and sends objects
  and answers the track's requests.
- `moq-wasm/media`: `publish()` sends a `MediaStream` as an MSF broadcast with
  LOC tracks; `LivePlayer` and `PlayerControls` play one back. Chrome, Edge and
  Safari. This entry point may change in any minor version.
