# mediapack

Pure-Rust media container demuxer/muxer and codec bitstream toolkit—no FFmpeg or GStreamer required.

Every demuxer emits `MediaEvent`s and every muxer consumes them back. Compiles on native targets and `wasm32`. Used by [`broadcast`](../broadcast/README.md), [`transcode`](../transcode/README.md), [`gst-plugin-moqt`](../gst-plugin-moqt/README.md), the SDKs, and the media/SIP/recorder services.

## Supported formats

| Format | Demux | Mux | Codecs | Notes |
| --- | --- | --- | --- | --- |
| MPEG-TS | `mpegts::Demuxer` | `mpegts::Muxer` | H.264, AAC | Supported by `Transmuxer` |
| FLV | `flv::Demuxer` | `flv::Muxer` | H.264, AAC | Supported by `Transmuxer` |
| fMP4 / MP4 | `mp4::Demuxer`, `mp4::Mp4Index` | `mp4::Fmp4Muxer`, `mp4::Fmp4TrackMuxer` | H.264, H.265, VP8, VP9, AV1, AAC, Opus, A-law, MP3 | `Fmp4Muxer` in `Transmuxer`; `Fmp4TrackMuxer` emits single-track CMSF fragments; `Mp4Index` indexes progressive MP4 |
| LOC (`draft-ietf-moq-loc-04`) | `loc::Demuxer` | `loc::Muxer` | H.264, H.265, VP8, VP9, AV1, AAC, Opus, A-law, MP3 | Object-based (outside `Transmuxer`) |
| RTP / RTCP / SDP | `rtp::Demuxer` | `rtp::Muxer` | H.264, AAC, Opus, A-law | Datagram-based (outside `Transmuxer`); muxer rejects MP3 |

## Install & Cargo features

```toml
[dependencies]
mediapack = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
```

| Feature | Enables |
| --- | --- |
| `moqt` | `loc::to_extension_headers`, `loc::from_extension_headers`, and `From<&moqt::TrackObject> for loc::LocObject` |
| `serde` | `Serialize` / `Deserialize` on `loc::LocExtension` and `loc::LocValue` |

## Quickstart

Run the bundled example to remux the MPEG-TS fixture to fragmented MP4:

```shell
cargo run -p mediapack --example transmux -- mpegts fmp4 crates/mediapack/fixtures/testsrc.ts out.mp4
```

In Rust (inside a function returning `anyhow::Result<()>`, given `ts_bytes: &[u8]`):

```rust
use mediapack::{InputFormat, OutputFormat, Transmuxer};

let mut transmuxer = Transmuxer::new(InputFormat::MpegTs, OutputFormat::Fmp4);
let mut out = transmuxer.push(ts_bytes)?.to_vec();
out.extend_from_slice(&transmuxer.finish()?);
```

## `MediaEvent` model

All timestamps are `Timestamp` values in microseconds:

| Variant | Contents |
| --- | --- |
| `MediaEvent::Streams(StreamSet)` | Tracks announced by the container |
| `MediaEvent::VideoConfig(VideoConfig)` | `Avc` / `Hevc` decoder config, or `Vp8` / `Vp9` / `Av1` config from a keyframe |
| `MediaEvent::AudioConfig(AudioConfig)` | `Aac(AudioSpecificConfig)`, or sample rate & channels for `Pcma` (G.711 A-law), `Opus`, or `Mp3` |
| `MediaEvent::Video(VideoSample)` | One Annex-B H.264/H.265 access unit (in-band parameter sets on keyframes), VP8/VP9 frame, or AV1 temporal unit |
| `MediaEvent::Audio(AudioSample)` | One raw audio frame (AAC without ADTS, A-law, Opus, or MP3) plus optional `AudioLevel` |

## Modules

| Module | Contents |
| --- | --- |
| `transmux` | `Transmuxer` (push API) and `transmux()` (`Read`/`Write` API) between `MpegTs`, `Flv`, and `Fmp4` |
| `mpegts`, `flv`, `mp4`, `loc`, `rtp` | Container and transport demuxers/muxers |
| `h264`, `hevc` | NAL unit parsing, SPS/VPS/PPS helpers, Annex-B ↔ AVCC/HVCC conversion |
| `vp8`, `vp9`, `av1`, `aac` | Keyframe detection, config extraction, `AudioSpecificConfig`, and ADTS parsing |
| `elementary_stream`, `parameter_sets` | `ElementaryStreamDemuxer` (raw Annex-B → access units) and `ParameterSetTracker` (prepends parameter sets to keyframes) |

## Key caveats

- **Always call `finish()`**: `Fmp4Muxer` and `Fmp4TrackMuxer` hold the last video sample until `finish()` to compute its duration; `mpegts::Demuxer`, `mp4::Demuxer`, and `rtp::Demuxer` flush pending access units on `finish()` (`flv::Demuxer` and `loc::Demuxer` have no `finish()`).
- **Static fMP4 config**: `Fmp4Muxer` and `Fmp4TrackMuxer` reject codec config changes after emitting the init segment.
- **LOC caller responsibilities**: `loc::Muxer` ignores `VideoConfig` events and normalizes H.264/H.265 Annex-B start codes to 4 bytes—callers feeding custom samples must include parameter sets in-band (e.g. via `ParameterSetTracker`). On the receive side, share one origin across `loc::Demuxer::video` and `loc::Demuxer::audio` via `with_capture_origin()`.
- **RTP loss & SSRC restarts**: Initialize with `rtp::Demuxer::new(&sdp)`. Before calling `demuxer.push(&datagram)`, check `if demuxer.is_new_source(&datagram)` and replace it with `demuxer.for_next_source()`. Reordering buffers up to 200 ms or 256 packets; on loss, H.264 drops until the next keyframe.
- **Buffer cap**: `mpegts::Demuxer` and `ElementaryStreamDemuxer` cap incomplete buffered data at 16 MiB.

## Test fixtures & further reading

- `fixtures/testsrc.{ts,flv,mp4}` contain 0.6 s of 160×90 H.264 + 48 kHz mono AAC; additional fixtures cover H.265, VP8, VP9, AV1, MP3, and progressive MP4.
- Architecture: [docs/architecture/mediapack/architecture.md](../../docs/architecture/mediapack/architecture.md)
- LOC specification: [`draft-ietf-moq-loc-04.txt`](../../docs/spec/draft-ietf-moq-loc-04.txt)
