# mediapack

Container and bitstream toolkit for the media that flows through this workspace.
It has no dependency on GStreamer or FFmpeg; everything is parsed and written in Rust.

| Format | Demux | Mux |
| --- | --- | --- |
| MPEG-TS | `mpegts::Demuxer` | `mpegts::Muxer` |
| FLV | `flv::Demuxer` | `flv::Muxer` |
| fMP4 | `mp4::Demuxer` | `mp4::Fmp4Muxer` |
| LoC | `loc::Demuxer` | `loc::Muxer` |

All four formats share H.264/AAC `MediaEvent`s. MPEG-TS, FLV, and fMP4 can
be converted in either direction through `Transmuxer`.

## Model

Demuxers turn container bytes or LoC objects into `MediaEvent`s; muxers produce
the corresponding container bytes or objects.

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
| `mpegts::Demuxer` / `mpegts::Muxer` | TS bytes and events; muxing writes PAT/PMT, PES, PCR, and ADTS |
| `flv::Demuxer` / `flv::Muxer` | FLV file streams and RTMP tag payloads (`push_tag`) |
| `mp4::Demuxer` / `mp4::Fmp4Muxer` | Fragmented MP4; muxing writes an init segment plus one fragment per sample |
| `mp4::Fmp4TrackMuxer` | One track per muxer with the init segment returned apart from the fragments, the shape draft-ietf-moq-cmsf-01 §3.1/§3.3 gives a MoQT track |
| `loc` | `Muxer`, `Demuxer`, and transport-independent `LocObject` with typed extensions |
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

LoC carries individual objects with extension headers and a payload, rather
than a byte stream. Use `loc::Muxer` and `loc::Demuxer` directly, outside the
`Read`/`Write` transmux API. Route video and audio to separate MoQ tracks;
provide the audio configuration from the catalog to `loc::Demuxer::audio`.

```rust
use mediapack::{MediaEvent, Timestamp, loc};

let muxer = loc::Muxer::new(Timestamp::from_micros(1_700_000_000_000_000));
let mut video_demuxer = loc::Demuxer::video();
for event in &events {
    if matches!(event, MediaEvent::Video(_)) {
        if let Some(object) = muxer.push(event) {
            let decoded_events = video_demuxer.push(&object)?;
        }
    }
}
```

The LoC muxer adds the capture origin to each sample's PTS and keeps H.264
payloads in Annex-B form with in-band parameter sets. Configuration events
produce no object. The demuxer measures PTS from the first capture timestamp
on its track and sets video DTS equal to PTS; separate decode timestamps and
cross-track clock alignment are not represented by this API.

The fMP4 demuxer reads H.264/AAC initialization segments and fragments with
explicit `trun` data offsets relative to `moof`, followed by `mdat`. This includes
the output of `Fmp4Muxer` and the FFmpeg fixture command below; unfragmented
MP4 is outside its scope. Call `finish()` to detect incomplete final input.
The MPEG-TS muxer emits complete packets on each `push()` and needs no flush.

## Fixtures

`fixtures/testsrc.ts` and `fixtures/testsrc.flv` are 0.6 s of ffmpeg `testsrc`
(160x90, H.264 baseline, 2 keyframes) with a mono 48 kHz AAC sine tone. They
drive the chunked demux and transmux tests.

`fixtures/testsrc.mp4` is a fragmented remux of `testsrc.ts`, generated from
within the fixtures directory:

```shell
ffmpeg -i testsrc.ts -c copy -bsf:a aac_adtstoasc \
  -movflags frag_keyframe+empty_moov+default_base_moof \
  -frag_duration 200000 testsrc.mp4
```
