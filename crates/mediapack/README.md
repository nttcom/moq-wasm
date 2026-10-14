# mediapack

Container and bitstream toolkit for the media that flows through this workspace.
It has no dependency on GStreamer or FFmpeg; everything is parsed and written in Rust.

| Format | Demux | Mux |
| --- | --- | --- |
| MPEG-TS | `mpegts::Demuxer` | `mpegts::Muxer` |
| FLV | `flv::Demuxer` | `flv::Muxer` |
| fMP4 | `mp4::Demuxer` | `mp4::Fmp4Muxer` |
| LoC | `loc::Demuxer` | `loc::Muxer` |
| RTP | `rtp::Demuxer` | `rtp::Muxer` |

All five formats share H.264/AAC `MediaEvent`s. MPEG-TS, FLV, and fMP4 can
be converted in either direction through `Transmuxer`.

## Model

Demuxers turn container bytes or LoC objects into `MediaEvent`s; muxers produce
the corresponding container bytes or objects.

| Event | Meaning |
| --- | --- |
| `Streams` | Which tracks the container announces (MPEG-TS PMT, FLV header flags) |
| `VideoConfig` | `AvcDecoderConfigurationRecord` (SPS/PPS, codec string) |
| `AudioConfig` | AAC `AudioSpecificConfig`, or the sample rate and channels of G.711 A-law, Opus or MP3; the FLV, MPEG-TS and MP4 muxers take AAC only |
| `Video` | One H.264 access unit in Annex-B form; keyframes carry SPS/PPS in-band |
| `Audio` | One encoded audio frame (a raw AAC access unit without ADTS header, A-law bytes, an Opus packet or an MP3 frame) |

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
| `rtp` | `Demuxer` / `Muxer` over RTP datagrams, `packet::Header` (RFC 3550), the H.264 (RFC 6184), AAC `MPEG4-GENERIC` (RFC 3640), G.711 A-law (RFC 3551) and Opus (RFC 7587) payload formats, and the `sdp::SessionDescription` subset that describes a plain RTP session |
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
on its track and sets video DTS equal to PTS; separate decode timestamps are
not represented by this API. To keep the tracks of one broadcast aligned,
give their demuxers one origin with `with_capture_origin`.

RTP carries datagrams rather than a byte stream, so `rtp::Demuxer` and
`rtp::Muxer` sit outside the transmux API too. The demuxer is built from the
session's SDP, which names the streams, their payload types and the out-of-band
parameter sets; feed it one datagram at a time from any of the session's ports.
The muxer tags each packet with its stream and writes the SDP once it has seen
the `Streams` event and the configuration of every announced stream.

```rust
use mediapack::rtp::{self, MediaKind};

let description = rtp::SessionDescription::parse(&sdp_text)?;
let mut demuxer = rtp::Demuxer::new(&description)?;
for datagram in datagrams {
    let events = demuxer.push(&datagram)?;
}

let mut muxer = rtp::Muxer::new();
for event in &events {
    for packet in muxer.push(event)? {
        match packet.kind {
            MediaKind::Video => video_socket.send(&packet.data)?,
            MediaKind::Audio => audio_socket.send(&packet.data)?,
        }
    }
}
let sdp_text = muxer.session_description(destination, 5004, 5006).unwrap().to_string();
```

Packets are put back in sequence order per stream; a gap that stays open while
200 ms of later packets arrive is reported through `discontinuities()`, and the
frames it cuts are dropped until the next keyframe. Timestamps restart at the
first packet of each stream, so the two streams are not aligned to each other
without RTCP, which this API does not carry.

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
