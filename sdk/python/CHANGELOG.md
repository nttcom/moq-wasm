# Changelog

## 0.1.0

- Breaking: the `codecs` extra (and so `pipecat`) requires `av>=15`, and the
  `pipecat` extra requires `pipecat-ai>=1.13,<2`.

- Breaking: a request made after the session ended raises what ended it
  (`SessionClosed`, `ConnectionLost` or `TokenExpired`, all subclasses of the
  new `moq_wasm.SessionEnded`) instead of `RuntimeError`; catch
  `SessionEnded` where you caught `RuntimeError` for a closed session.
  `Session.run` and `close()` return only once the session's cleanup
  finished, and entering one `connect()` result twice at once raises
  `RuntimeError`.

- Breaking: without `refresh_token`, the session ends with `ConnectionLost`
  when the relay closes it at the token's expiry, instead of reconnecting with
  the expired token. With it, the token is refreshed five minutes before
  expiry (or halfway through a shorter lifetime) and failed refreshes are
  retried.

- Breaking: `moq_wasm.Subscription`, `Group` and `Fetch` are SDK types
  wrapping the native ones. A read while another waits raises `RuntimeError`,
  and so does reading groups after objects (or the reverse). A read cancelled
  by a timeout no longer loses the object it took, and a request cancelled
  before it returns undoes what the relay accepted. `Subscription.close()`
  sends UNSUBSCRIBE at once.

- Breaking: `NamespaceWatch.close()` is a coroutine that sends
  UNSUBSCRIBE_NAMESPACE, so the prefix can be watched again; await it.

- Breaking: a track has one `TrackWriter` and a namespace one announcement per
  session: `publish_track` or `announce` raises `RuntimeError` until the
  earlier one's `close()` finished, and `publish_track` with another
  `retention_seconds` than the first registration raises `ValueError`.
  `TrackWriter` takes the session as its first argument. The new
  `TrackWriter.close()` ends the track's subscriptions with PUBLISH_DONE, and
  `registered()` waits for the registration.

- Breaking: `LocalBroadcast` holds its namespace and tracks until it is left:
  - Entering it raises `RuntimeError` while another announcement or broadcast
    of the session publishes the namespace, and after it was left
    `publish_track`, `event_timeline`, `add_media_source` and its writers
    raise. Create a new broadcast instead.
  - A media broadcast claims its catalog, LOC, `<video>_timeline`, CMAF,
    `chat` and `audio`-placeholder tracks; give raw tracks other names.
  - Leaving a block that exited normally raises the namespace refusal unless
    it was raised before; leaving ends the subscribers' media and catalog
    subscriptions, even when cancelled.
  - `LocalBroadcast.publish_track` takes `priority`.

- Breaking: media sources check their input:
  - Invalid options raise `ValueError` or `TypeError` from
    `add_media_source`, `add_rendition` and `configure_audio`, and
    `MediaSource.video`, `MediaSource.audio` and `Rendition.name` are
    read-only.
  - Frames pushed before the relay accepted the namespace are dropped:
    `await broadcast.published()` before pushing. `push_video`, `push_audio`
    and `Rendition.push_video` return whether the frame was taken.
  - Once native publishing failed, pushes and configuration raise
    `RuntimeError` instead of logging; rebuild the broadcast.
  - A second concurrent `keyframe_request()` or `demand_change()` raises
    `RuntimeError`.

- Breaking: `RemoteBroadcast.catalogs()`, `follow()` and `event_timeline()`
  return a `moq_wasm.media.Following` instead of an async generator: a
  cancelled read no longer ends the iteration. Read them in
  `contextlib.aclosing(...)`; `async for` keeps working.
  `RemoteBroadcast.subscriptions(name, ...)` and `on_subscribed=` are new.

- Breaking: `watch_broadcasts` raises what ended the session (`SessionEnded`)
  instead of returning, and cancels and waits for its handlers before it
  ends. A republished broadcast's handler starts only after the previous one
  finished.

- Breaking: `Catalog.parse` raises `ValueError` for a catalog without a
  `tracks` array (draft-ietf-moq-msf-02 §5.1.4). `Catalog.init_data(track)`
  returns a track's inline initialization data, and `Catalog.fields` holds
  every root field.

- Breaking: `VideoDecoder.decode(payload, location=None)` returns a list of
  `(picture, location)` pairs, every picture the object completes in
  presentation order, and `flush()` returns the pictures held back.
  `decode_pictures` returns a `Following` and yields every picture of each
  group (B-frames included):

  ```python
  # before
  picture = decoder.decode(payload)
  # after
  for picture, _ in decoder.decode(payload): ...
  ```

- Breaking: `OpusEncoder.encode` raises `ValueError` for a sample rate that
  is not a positive `int` (such as `16000.0`); it now takes a rate that
  changes between calls, and empty PCM. `VideoEncoder.encode` takes `BGR` and
  `BGRA` and raises `ValueError` for another format or a mismatched size.

- Breaking: the chat track's `eventType` is now
  `io.skyway.research.moqt.chat.v1` (was `com.skyway.chat.v1`). Accept both
  names to read the chat of publishers on 0.0.1.

- Breaking: pipecat (`MoqWasmTransport`):
  - It raises `ValueError` for `audio_out_destinations` /
    `video_out_destinations`, `audio_in_filter`, `audio_out_channels` or
    `audio_in_channels` other than 1 or 2, `SoundfileMixer` with stereo
    output, and a nonpositive `audio_out_10ms_chunks`. Run one transport per
    broadcast and one `MoqWasmParticipantInput` per filtered participant.
  - The output encodes Opus at `audio_out_bitrate` (pipecat's default
    96 kbit/s, formerly a fixed 32 kbit/s; pass `audio_out_bitrate=32_000` to
    keep it) and video at `video_out_bitrate`, and publishes stereo with
    `audio_out_channels=2`.
  - A room that cannot open fires `on_error` and leaves the input and output
    unusable; build the `PipelineWorker` with
    `processor_unusable_policy=ProcessorUnusablePolicy.CANCEL` (or `END`).
  - Each processor serves one pipeline run: build each pipeline from
    `input()` / `output()` and a new `MoqWasmParticipantInput`.
  - `ParticipantSink.push_picture` and `MoqWasmTransport.push_picture` return
    whether the picture was passed on.

- pipecat behavior:
  - Speech is written packet by packet at most 100 ms ahead, so an
    interruption stops at the next packet, and the end of an utterance is no
    longer lost at `TTSStoppedFrame`, `EndFrame`, `StopFrame` or a pause.
  - Pictures are written synchronized with the audio, and track messages as
    they reach the output, in frame order.
  - A failed media or message write is a non-fatal `ErrorFrame`; a failed
    participant track is received again with backoff up to 30 s.
  - An image request no picture answers within 5 s gets `{"error": ...}`.
  - `MoqWasmTransport.get_client_id` returns the participant id.

- `moq_wasm.codecs.follow_pictures(remote, name, codec)` follows a video track
  across its subscriptions, decoding each from the start of the current group
  and carrying the Video Config over. `VideoDecoder` and `decode_pictures`
  take `config=` (the LOC Video Config, e.g. an avcC record), decode AV1, and
  `OpusEncoder` / `OpusDecoder` take a channel count.

- `RemoteBroadcast.event_timeline(name, ...)` yields the records of an MSF
  event timeline track, starting with the group the relay holds, without a
  record twice across resubscriptions. `Session.subscribe_current_group`
  takes `priority`, `delivery_timeout` and `group_order`.

- `Session.close()` waits up to 2 s until the relay has acknowledged the
  groups the session finished writing, so a group written just before
  closing is no longer lost.

- `moq_wasm.connect` waits up to 30 s (was 10 s) for a `wss://` relay or the
  WebSocket fallback, fails when no SERVER_SETUP arrives within 30 s, and
  logs a `session connect failed` warning with the reason.

- A media source with `on_demand=True` and `cmaf=True` reports the demand of
  its AAC CMAF track, and `MediaSource.demand_change()` keeps only each
  track's latest unread change.

- `retention_seconds` is honoured on a track no longer written to, and a
  track writer that never calls `next_subscriber` no longer holds memory per
  SUBSCRIBE. Renditions drop frames up to the next keyframe beyond about 300
  queued frames instead of growing memory.

- `session.publish_track` with `datagram=True` accepts a track registered
  again; without it, a track that sent datagrams raises `ValueError`. A
  refused PUBLISH raises `RequestError`. A SUBSCRIBE to a track
  `remove_track` removed is refused with `TRACK_DOES_NOT_EXIST`.

- Telemetry and log delivery no longer stop after one failure, and a NaN or
  unserializable field is sent as its `str`.

- The wheel carries the MIT license, the README and the project's URLs.

- Fixed:
  - exiting a `create_broadcast` block right after publishing sometimes
    hanging;
  - a subscriber missing objects of overlapping or cancelled
    `TrackWriter.write` calls;
  - `Subscription.done()` not resolving when PUBLISH_DONE arrives right after
    SUBSCRIBE_OK;
  - on CPython 3.14.0 to 3.14.6, finished tasks being kept until the session
    ends;
  - a dropped unfinished group ending with a FIN, so subscribers took it for
    complete;
  - an H.264/H.265 frame without a slice being published;
  - media publishing stopping, or CMAF taking another group id than LOC,
    when a frame repeated the previous timestamp, and CMAF samples
    overlapping by one tick.

- GOAWAY migration, protocol limits and malformed input: after a GOAWAY a
  subscription no longer drops or duplicates a group, and a session closed
  during the migration stays closed (#2073, #2074); a request with more than
  32 namespace fields, an empty field on draft-18, or a Full Track Name over
  4,096 bytes raises `RequestError` instead of closing the session (#1740,
  #1812); draft-18 request edge cases, NEW_GROUP_REQUEST throttling and Prior
  Group ID Gap are handled (#2071, #2072, #2085); and draft-14 parsing edge
  cases are handled (#1707, #1738).

## 0.0.1

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
