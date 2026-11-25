# GStreamer for ONVIF RTSP Input (`ingest-gateway`)

**Status:** Accepted (2026-10-09)

## What

Replace `ffmpeg-next` with `gstreamer` and `gstreamer-app` in `ingest-gateway pull`. A pipeline of `rtspsrc`, RTP depayloaders, `h264parse`, and `appsink` receives the camera's RTSP stream and delivers H.264 access units and audio frames to the bridge.

## Context

IP cameras place H.264 parameter sets differently: in-band before every IDR, out-of-band in SDP `sprop-parameter-sets`, or both.

The previous FFmpeg input passed Annex-B packets through unchanged while attaching its own `avcC` as the LOC Video Config extension. On cameras sending in-band SPS/PPS (such as Tapo C2xx), browser WebCodecs decoders failed on every keyframe because a `description` makes the decoder expect length prefixes (`draft-ietf-moq-loc-04` §2.1). Dropping the extension without injecting SDP parameter sets breaks late joiners on SDP-only cameras.

Every other LOC publisher in this workspace (browser, `ingest-gateway`, `gst-plugin-moqt`) sends Annex-B with in-band parameter sets and no Video Config, and `gst-plugin-moqt` and `transcode` already depend on GStreamer.

## Alternatives

- **Keep FFmpeg, drop Video Config in Annex-B mode, and seed `mediapack::h264::ParameterSetTracker` from SDP extradata:** Smaller diff, but requires a new seeding entry point, and FFmpeg delivers extradata in Annex-B rather than the `avcC` format the old code parsed.
- **Keep FFmpeg and convert Annex-B to length prefixes in the player when `description` is present:** Valid per LOC, but fixes only this player and leaves `ingest-gateway pull` inconsistent with every other publisher.

## Decision

Use GStreamer:

- `h264parse config-interval=-1` injects SPS/PPS (including those from SDP) before every IDR, producing byte-stream Annex-B with no `avcC` so every group is decodable by late joiners.
- `rtspsrc` handles RTSP over TCP, authentication, and the RTP jitter buffer.
- The bridge still runs `ParameterSetTracker` on access units to derive the codec string.

## Consequences

- `ingest-gateway pull` requires the GStreamer runtime with `gst-plugins-good` (`rtspsrc`, RTP depayloaders) and `gst-plugins-bad` (`h264parse`) instead of FFmpeg.
- `ffmpeg-next` is removed from the workspace; CI no longer installs FFmpeg dev packages or `libclang`.
- When a camera also sends in-band SPS/PPS, keyframes carry parameter sets twice (`h264parse`'s copy, then the camera's). H.264 allows repeated parameter sets, so decoders handle this cleanly.
