# mediapack

Pure-Rust container demuxer/muxer and bitstream toolkit (no GStreamer or FFmpeg). Demuxers turn container bytes or LOC objects into `MediaEvent`s, and muxers serialise them back.

## Supported formats

| Format | Demux | Mux | Notes |
| --- | --- | --- | --- |
| MPEG-TS | `mpegts::Demuxer` | `mpegts::Muxer` | Supported by `Transmuxer` |
| FLV | `flv::Demuxer` | `flv::Muxer` | Supported by `Transmuxer` |
| fMP4 | `mp4::Demuxer` | `mp4::Fmp4Muxer` | Supported by `Transmuxer`; also `mp4::Fmp4TrackMuxer` (single-track segments + separate init) and `mp4::Mp4Index` (progressive MP4 sample index) |
| LOC | `loc::Demuxer` | `loc::Muxer` | Object-based (outside `Transmuxer`) |
| RTP | `rtp::Demuxer` | `rtp::Muxer` | Datagram-based (outside `Transmuxer`) |

All five formats support H.264/AAC; LOC and `mp4::Fmp4Muxer` also support H.265, VP8, VP9, and AV1.

Optional Cargo features:

- `moqt`: maps LOC properties to MoQT extension headers.
- `serde`: derives `Serialize`/`Deserialize` for LOC extension values (`loc::LocExtension`, `loc::LocValue`).

## Quickstart

Try the bundled example to remux the MPEG-TS fixture to fMP4:

```shell
cargo run -p mediapack --example transmux -- mpegts fmp4 crates/mediapack/fixtures/testsrc.ts out.mp4
```

In Rust, write bytes from both `push()` and `finish()` (the fMP4 muxer buffers the last video sample until `finish()` to compute its duration):

```rust
use std::io::Write;
use mediapack::{InputFormat, OutputFormat, Transmuxer};

let mut muxer = Transmuxer::new(InputFormat::MpegTs, OutputFormat::Fmp4);
output.write_all(&muxer.push(&ts_bytes)?)?;
output.write_all(&muxer.finish()?)?;
```

## `MediaEvent` model

All timestamps are `Timestamp` values in microseconds:

| Event | Meaning |
| --- | --- |
| `Streams` | Tracks announced by the container |
| `VideoConfig` | `Avc`/`Hevc` decoder config, or VP8/VP9/AV1 config from a keyframe |
| `AudioConfig` | AAC `AudioSpecificConfig`, or sample rate/channels for G.711 A-law, Opus, or MP3 |
| `Video` | One H.264/H.265 Annex-B access unit (in-band parameter sets), VP8/VP9 frame, or AV1 temporal unit |
| `Audio` | One encoded audio frame (raw AAC without ADTS, A-law, Opus, or MP3) |

## Format notes

- **LOC**: Construct per-track demuxers from the catalog codec (`loc::Demuxer::video`, `loc::Demuxer::audio`) and share an origin via `with_capture_origin` so video and audio stay aligned.
- **RTP**: Initialise the demuxer from SDP and feed one datagram at a time; a gap is reported as lost via `discontinuities()` once buffered later packets' timestamps span 200 ms (or 256 packets are pending), dropping frames until the next keyframe for H.264 video (AAC resets its depacketizer; A-law and Opus resume on the next packet). The muxer emits SDP (`session_description`) and RTCP sender reports (`sender_reports`) for A/V sync.

## Bitstream & helper modules

| Module | Contents |
| --- | --- |
| `h264`, `hevc` | NAL unit types, SPS parsing, Annex-B/AVCC conversion |
| `elementary_stream` | `ElementaryStreamDemuxer` (splits raw Annex-B into access units) |
| `parameter_sets` | `ParameterSetTracker` (tracks config changes and prepends missing parameter sets to keyframes) |
| `vp8`, `vp9`, `av1` | Keyframe detection and config extraction |
| `aac` | `AudioSpecificConfig` and ADTS parsing |
| `transmux` | `Transmuxer` (push API) and `transmux()` (`Read`/`Write` API) |

## Fixtures

`fixtures/testsrc.ts` and `fixtures/testsrc.flv` contain 0.6 s of ffmpeg `testsrc` (160x90 H.264 baseline, 2 keyframes) + mono 48 kHz AAC sine tone. `fixtures/testsrc.mp4` is a fragmented remux of `testsrc.ts`:

```shell
ffmpeg -i testsrc.ts -c copy -bsf:a aac_adtstoasc \
  -movflags frag_keyframe+empty_moov+default_base_moof \
  -frag_duration 200000 testsrc.mp4
```

Design: [architecture.md](../../docs/architecture/mediapack/architecture.md).
