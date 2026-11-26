# Changelog

## 0.1.0

- Breaking: `moqt::Subscription` has a new `end: SubscriptionEnd` field; code
  building it with a struct literal adds `end: SubscriptionEnd::default()`.

- Breaking: `Subscriber::subscribe_update` and
  `moqt::subscribe::SubscribeUpdateOption` are removed; use
  `Subscription::set_priority` or `end_before_group`.

- Breaking: an object of a track with `Retention::For(duration)` expires once
  its age reaches `duration`, also when the track is no longer written to, so
  `Retention::For(Duration::ZERO)` caches nothing; use `Retention::NewestOnly`
  to keep the newest object.

- `moq_wasm::robotics` runs a robot room: `run_robot(session, broadcast,
  config)` runs the `Robot` of the `<room>/robot` broadcast, and
  `offer_actions(broadcast)` returns a `Driver` that sends `action` chunks
  from an operator or policy broadcast. `Robot`, `Driver`, `Command`,
  `HoldReason`, `ArbiterConfig`, `State` and `decode_object` are re-exported.

- New on the session (`moqt::client::Session`):
  - `follow_track(namespace, track)` returns a `TrackFollower` that follows a
    track across the publishers that serve it.
  - `watch_tracks(prefix)` returns a `TrackWatch` of the namespaces and
    PUBLISH requests one SUBSCRIBE_NAMESPACE brings; `TrackWatch::next`
    returns `None` once the session ends.
  - `PublishTrackOptions::priority` sets a track's Publisher Priority
    (default 128).
  - `moqt_session().publishes_namespace(namespace)` tells whether the session
    publishes a namespace, and `Publisher::begin_publish_subscribed_namespace`
    sends a namespace only on the peer's SUBSCRIBE_NAMESPACE.

- `moq_wasm::connect` fails when the relay accepts the connection but sends no
  SERVER_SETUP within 30 s, instead of waiting forever.

- `moq_wasm::media::Catalog::parse` returns `CatalogParseError::Malformed` for
  a catalog without a `tracks` array (draft-ietf-moq-msf-02 §5.1.4), and the
  media timeline lists each group's capture time in Unix milliseconds, the
  LOC Timestamp's clock (§7.1.1).

- A SUBSCRIBE to a track `MediaSource::remove_track` removed gets
  `TRACK_DOES_NOT_EXIST` until the track is published again; FETCH still
  returns its cached objects.

- Memory no longer grows without bound: `Rendition::push_video` drops frames
  up to the next keyframe beyond about 300 queued frames (and
  `MediaSource::keyframe_request` resolves), `MediaSource::demand_change`
  takes `&self` and keeps only each track's latest unread change, and a
  `TrackWriter` that never calls `next_subscriber` no longer holds memory per
  SUBSCRIBE.

- Fixed:
  - `Subscription::done` not resolving when PUBLISH_DONE arrives right after
    SUBSCRIBE_OK;
  - on draft-18, a PUBLISH the peer accepted and at once unsubscribed failing
    with "request stream reset";
  - answering a request handler twice sending both answers;
  - `follow_track` resubscribing to a publisher that withdrew its namespace.

- GOAWAY migration and protocol details: after a GOAWAY a subscription no
  longer drops or duplicates a group, and a session closed during the
  migration stays closed (#2073, #2074); NEW_GROUP_REQUEST throttling and
  Prior Group ID Gap are sent correctly (#2071, #2072).

## 0.0.1

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
