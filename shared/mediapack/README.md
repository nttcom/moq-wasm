# mediapack

Container and bitstream toolkit for the media that flows through this workspace.
It has no dependency on GStreamer or FFmpeg; everything is parsed and written in Rust.

```
RTMP tags / FLV bytes ─┐                      ┌─> H.264 + AAC samples (MoQT publishers)
                       ├─> MediaEvent stream ─┼─> FLV   (flv::Muxer)
SRT / MPEG-TS bytes ───┘                      └─> fMP4  (mp4::Fmp4Muxer)
```

## Model

Every demuxer turns bytes into `MediaEvent`s and every muxer turns `MediaEvent`s back
into bytes, so any input can feed any output.

| Event | Meaning |
| --- | --- |
| `Streams` | Which tracks the container announces (MPEG-TS PMT, FLV header flags) |
| `VideoConfig` | `AvcDecoderConfigurationRecord` (SPS/PPS, codec string) |
| `AudioConfig` | `AudioSpecificConfig` (object type, sample rate, channels) |
| `Video` | One H.264 access unit in Annex-B form; keyframes carry SPS/PPS in-band |
| `Audio` | One raw AAC access unit without ADTS header |

Timestamps are `Timestamp` values in microseconds with conversions to and from
90 kHz ticks, milliseconds, and arbitrary timescales.

## Modules

| Module | Contents |
| --- | --- |
| `mpegts::parser` | TS packet alignment and parsing, PAT/PMT sections, PES headers |
| `mpegts::Demuxer` | Push-based demuxer: TS bytes in, `MediaEvent`s out |
| `flv::Demuxer` / `flv::Muxer` | FLV file streams and RTMP tag payloads (`push_tag`) |
| `mp4::Fmp4Muxer` | Init segment plus one `moof`/`mdat` fragment per sample |
| `h264` | Annex-B/AVCC conversion, NAL unit types, SPS parsing (codec string, dimensions) |
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

The `fMP4` muxer emits the init segment once every announced track has its
configuration and holds back one video sample so each fragment can carry the
sample duration. Call `finish()` to flush the last sample.

```shell
cargo run -p mediapack --example transmux -- mpegts fmp4 in.ts out.mp4
```

## Fixtures

`fixtures/testsrc.ts` and `fixtures/testsrc.flv` are 0.6 s of ffmpeg `testsrc`
(160x90, H.264 baseline, 2 keyframes) with a mono 48 kHz AAC sine tone. They
drive the chunked demux and transmux tests.
