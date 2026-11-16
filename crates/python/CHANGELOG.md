# Changelog

## Unreleased

- The first release of `moq-wasm` for Python, as abi3 wheels for Python 3.11
  and later on Linux x86_64, Linux aarch64 and macOS arm64. The session offers
  draft-18, then draft-14.

- `moq_wasm.connect` opens a session with the method names of the browser
  SDK's `MoqSession`. It follows GOAWAY, replaces a lost connection for up to
  60 s and refreshes the token through `refresh_token`.

- `session.subscribe` takes the browser SDK's options (filter, forward,
  priority, delivery timeout, group order); `subscribe_group_start`, `fetch`
  and `track_status` are added. A subscription reads its objects by group
  (`groups()`), and `done()`, `set_forward`, `set_priority`,
  `request_new_group` and `end_before_group` match the browser SDK.

- `session.publish_track` takes `retention_seconds`, `publish` (send PUBLISH)
  and `datagram`. `write` returns the group id the object went into,
  `write_group` sends a group of its own and `next_subscriber` resolves once a
  SUBSCRIBE is answered.

- A failed request raises `moq_wasm.RequestError` with the draft's
  `message_type`, `code` and `reason`; `ConnectionLost` carries how the
  session closed as `close_info`.

- `moq_wasm.media`: `watch_broadcasts` and `RemoteBroadcast` follow MSF
  broadcasts, and `create_broadcast` publishes a media source with its
  catalog: H.264 video with renditions (`add_rendition`), Opus or AAC audio
  with audio levels, CMAF next to LOC (`cmaf=True`), tracks sent only while
  subscribed (`demand_change`), and `remove_track`.

- `moq_wasm.pipecat.MoqWasmTransport` replaces pipecat's `MOQTransport`
  (`pipecat` extra); `moq_wasm.codecs` wraps PyAV's Opus, H.264 and VP8
  (`codecs` extra).

- `moq_wasm.init` forwards the native module's logs to `logging` and its
  spans to `on_span`, and `telemetry=TelemetryOptions(...)` sends both to an
  observability server, as the browser SDK's `init` does.
