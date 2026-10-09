# GStreamer for the RTSP input of the ONVIF bridge

## Status

Accepted

## Date

2026-10-09

## What

Replace `ffmpeg-next` with the `gstreamer` and `gstreamer-app` crates in
`onvif-ingest`. A pipeline of `rtspsrc`, the RTP depayloaders, `h264parse` and
`appsink` receives the camera's RTSP stream and hands H.264 access units and
audio frames to the bridge.

## Context

Cameras differ in where they put the H.264 parameter sets: some repeat SPS/PPS
in front of every IDR, some announce them only in the SDP
(`sprop-parameter-sets`), and some do both. The FFmpeg input passed the
camera's Annex-B packets through as received and attached its own avcC as the
LOC Video Config extension. With a Tapo C2xx camera, which carries SPS/PPS in
band, that combination meant every browser decoder failed on every keyframe: a
WebCodecs `description` makes the decoder read length prefixes
(draft-ietf-moq-loc-01 §2.1). Without the extension, a camera that sends the
parameter sets only in the SDP produces groups no late joiner can decode.

Every other LOC publisher in the repository (browser, `live-ingest`,
`gst-plugin-moqt`) sends Annex-B with the parameter sets in band and no Video
Config, and `gst-plugin-moqt` and `transcode` already depend on GStreamer.

## Alternatives

- Keep FFmpeg, drop the Video Config in Annex-B mode, and run the packets
  through `mediapack::h264::ParameterSetTracker` seeded from the SDP extradata.
  Smaller diff, but the tracker needs a new seeding entry point, and FFmpeg's
  extradata arrives in Annex-B form even though the old code parsed it as an
  avcC.
- Keep FFmpeg and fix the player to convert Annex-B to length prefixes when a
  description is present. Correct per LOC, but it only fixes this player, and
  the bridge would still be the one LOC publisher that differs from the rest.

## Decision

Use GStreamer. `h264parse config-interval=-1` inserts SPS/PPS before every IDR,
including those taken from the SDP, so every group starts decodable whatever
the camera does, and the byte-stream output carries no avcC. `rtspsrc` handles
RTSP over TCP, authentication and the RTP jitter buffer. The bridge still runs
`ParameterSetTracker` on the access units for the codec string and, in
`--payload-format avcc` mode, the avcC, as `transcode` does.

## Consequences

- `onvif-ingest` needs GStreamer at run time with the good (`rtspsrc`, RTP
  depayloaders) and bad (`h264parse`) plugins, instead of the FFmpeg libraries.
- `ffmpeg-next` is no longer used anywhere in the workspace; CI can stop
  installing the FFmpeg development packages and libclang in a follow-up.
- When a camera also sends SPS/PPS in band, keyframes carry them twice
  (h264parse's copy, then the camera's). H.264 allows repeated parameter sets,
  so decoders are unaffected.
