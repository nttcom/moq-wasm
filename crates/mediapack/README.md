# mediapack

Container and bitstream toolkit for the media that flows through this
workspace, written in Rust without GStreamer or FFmpeg.

| Format | Demux | Mux |
| --- | --- | --- |
| MPEG-TS | `mpegts::Demuxer` | `mpegts::Muxer` |
| FLV | `flv::Demuxer` | `flv::Muxer` |
| fMP4 | `mp4::Demuxer` | `mp4::Fmp4Muxer` |
| LoC | `loc::Demuxer` | `loc::Muxer` |
| RTP | `rtp::Demuxer` | `rtp::Muxer` |

Demuxers turn container bytes or LoC objects into `MediaEvent`s and muxers
turn them back. All five formats carry H.264/AAC; LoC and `mp4::Fmp4Muxer`
also carry H.265, VP8, VP9 and AV1. MPEG-TS, FLV and fMP4 convert in either
direction through `Transmuxer`.

| Event | Meaning |
| --- | --- |
| `Streams` | Which tracks the container announces |
| `VideoConfig` | The decoder configuration: `Avc` / `Hevc` records, or the VP8, VP9 or AV1 configuration read from a keyframe |
| `AudioConfig` | AAC `AudioSpecificConfig`, or the sample rate and channels of G.711 A-law, Opus or MP3 |
| `Video` | One H.264/H.265 access unit in Annex-B form with in-band parameter sets, a VP8/VP9 frame or an AV1 temporal unit |
| `Audio` | One encoded audio frame (AAC without ADTS header, A-law bytes, an Opus packet or an MP3 frame) |

Timestamps are `Timestamp` values in microseconds.

## Modules

| Module | Contents |
| --- | --- |
| `mpegts`, `flv`, `mp4` | Demuxers and muxers; `mp4::Fmp4TrackMuxer` writes one track with its init segment apart (draft-ietf-moq-cmsf-01 §3.1/§3.3) |
| `loc` | `Muxer`, `Demuxer` and the transport-independent `LocObject` with typed extensions |
| `rtp` | `Demuxer` / `Muxer` over RTP datagrams, the H.264, AAC, G.711 A-law and Opus payload formats, and `sdp::SessionDescription` |
| `h264`, `hevc` | NAL unit types, SPS parsing, Annex-B/AVCC conversion; `h264::ElementaryStreamDemuxer` splits a raw Annex-B stream into access units |
| `parameter_sets` | `ParameterSetTracker`: reports a changed configuration and completes a keyframe that lacks its parameter sets |
| `vp8`, `vp9`, `av1` | Keyframe detection and the configuration a keyframe carries |
| `aac` | AudioSpecificConfig and ADTS parsing |
| `transmux` | `Transmuxer` (push API) and `transmux()` (Read/Write API) |

## Usage

```rust
use mediapack::{InputFormat, MediaEvent, OutputFormat, Transmuxer, mpegts};

let mut demuxer = mpegts::Demuxer::new();
for event in demuxer.push(&ts_bytes)? {
    if let MediaEvent::Video(sample) = event {
        publish(sample.data, sample.is_keyframe, sample.pts);
    }
}

let mut transmuxer = Transmuxer::new(InputFormat::MpegTs, OutputFormat::Fmp4);
let fragment_bytes = transmuxer.push(&ts_bytes)?;
```

```shell
cargo run -p mediapack --example transmux -- mpegts fmp4 in.ts out.mp4
```

- **fMP4**: the muxer holds back one video sample to know each sample's
  duration, so call `finish()` to flush the last one. The demuxer reads
  fragmented H.264/AAC only, not unfragmented MP4.
- **LoC** carries objects, not a byte stream, so `loc::Muxer` and
  `loc::Demuxer` sit outside the transmux API. Route video and audio to
  separate tracks and give the demuxer the codec from the catalog
  (`loc::Demuxer::video`, `loc::Demuxer::audio`). To keep the tracks of one
  broadcast aligned, give their demuxers one origin with
  `with_capture_origin`.
- **RTP** carries datagrams, so it sits outside the transmux API too. The
  demuxer is built from the session's SDP and fed one datagram at a time; a
  gap still open after 200 ms of later packets is reported through
  `discontinuities()`, and frames are dropped until the next keyframe. The
  muxer writes the SDP (`session_description`) and RTCP sender reports
  (`sender_reports`), by which a receiver aligns video and audio.

## Fixtures

`fixtures/testsrc.ts` and `fixtures/testsrc.flv` are 0.6 s of ffmpeg `testsrc`
(160x90, H.264 baseline, 2 keyframes) with a mono 48 kHz AAC sine tone.
`fixtures/testsrc.mp4` is a fragmented remux of `testsrc.ts`:

```shell
ffmpeg -i testsrc.ts -c copy -bsf:a aac_adtstoasc \
  -movflags frag_keyframe+empty_moov+default_base_moof \
  -frag_duration 200000 testsrc.mp4
```
