# transcode

In-process GStreamer video processing over [`mediapack::MediaEvent`](../mediapack/README.md):

1. **`Transcoder`**: Live H.264 adaptive-bitrate (ABR) ladder transcoder with keyframe alignment (`draft-ietf-moq-cmsf-01` §3.2), used by `ingest-gateway`.
2. **`Compositor`**: Offline multi-participant MP4 compositor with active-speaker layout, used by `recorder`.

Native-only; requires GStreamer 1.20+.

## Install & GStreamer plugins

```toml
[dependencies]
transcode = { git = "https://github.com/nttcom/moq-wasm", rev = "<commit>" }
```

Install GStreamer 1.20+ (`brew install gstreamer` on macOS; on Debian/Ubuntu install `pkg-config`, `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, and the plugin packages below):

| GStreamer package | Needed for `Transcoder` | Needed for `Compositor` |
| --- | --- | --- |
| `gst-plugins-base` | `appsrc`, `appsink`, `decodebin`, `videoconvert`, `videoscale`, `tee`, `queue` | `appsrc`, `decodebin3`, `compositor`, `audiomixer`, `audioconvert`, `audioresample`, `videoconvert`, `videoscale`, `opusdec` |
| `gst-plugins-good` | — | `isomp4` (`mp4mux`), `level`, `vpx`, `alaw`, `mpg123`, `audioparsers` |
| `gst-plugins-bad` | `h264parse` | `h264parse`, `av1dec` (or `dav1ddec`), `av1parse`, `opusparse` |
| `gst-plugins-ugly` | `x264enc` | `x264enc` |
| `gst-libav` | H.264 decoder (or platform decoder) | `avdec_h264`, `avenc_aac` |

## Live ABR transcoding (`Transcoder`)

Decodes an H.264 Annex-B stream once and encodes lower-resolution H.264 baseline renditions whose keyframes match the source PTS. Audio is not transcoded—forward audio `MediaEvent`s directly.

Try the bundled example on the MPEG-TS test fixture:

```shell
cargo run -p transcode --example transcode -- crates/mediapack/fixtures/testsrc.ts out 54:100
```

Finite-input Rust example (inside an `async fn` returning `anyhow::Result<()>`):

```rust
use transcode::{Transcoder, ladder_for};

// Standard ladder (1080p/720p/480p/360p) strictly below source height; empty for <=360p.
let renditions = ladder_for(source_sps.width, source_sps.height);
if renditions.is_empty() {
    return Ok(()); // Transcoder::new rejects an empty slice
}

let mut transcoder = Transcoder::new(&renditions)?;
let input = transcoder.input(); // Cloneable; push from another task/thread for live streams
for sample in &video_samples {
    input.push(sample)?;
}
input.finish()?;

while let Some(output) = transcoder.next().await {
    let output = output?; // output.rendition indexes `renditions`
    publish(output.rendition, output.event); // VideoConfig once per rendition, then Video
}
```

> **Live streams**: Drain `transcoder.next().await` concurrently with `input.push(...)` (for example in a spawned task). Live input never finishes and the output channel is unbounded.

## Offline MP4 compositing (`Compositor`)

Mixes participant recordings into one progressive `faststart` MP4 (1280×720 @ 15 fps H.264 2 Mbps + stereo 48 kHz AAC 128 kbps), placing a screen share or active speaker large on the left and other participants in a right-hand strip. Inside an `async fn` returning `anyhow::Result<()>`:

```rust
use transcode::{CompositeInput, Compositor, Loudness};

let loudness = Loudness::measure(audio_events_pass1)?; // blocking decode pass
let inputs = vec![CompositeInput {
    video: Some(video_events),
    screen_share: None,
    audio: Some(audio_events_pass2),
    loudness: Some(loudness),
}];
let mut compositor = Compositor::start(inputs, output_path)?;
compositor.finished().await?; // dropping `compositor` cancels the job
```

## Key caveats

- **Codecs**: `Transcoder` accepts H.264 Annex-B only (`x264enc tune=zerolatency speed-preset=veryfast`). `Compositor` accepts H.264, H.265, VP8, VP9, AV1 video (including mid-stream resolution changes) and AAC, Opus, A-law, MP3 audio.
- **Backpressure & cleanup**: Input `appsrc` blocks at 4 MiB; dropping `Transcoder` or `Compositor` transitions the GStreamer pipeline to `Null`.
- **Active speaker & hiding**: `Loudness::measure` computes 100 ms RMS windows (1 s speech to switch speaker, 1.5 s pause hold). Tiles hide after > 1 s camera gap or > 5 s screen-share gap.
- **Compositor inputs**: `Compositor::start` requires a UTF-8 `output` path and at least one non-empty input stream.

## Further reading

- GStreamer usage note: [docs/architecture/transcode/gstreamer.md](../../docs/architecture/transcode/gstreamer.md)
- Callers: [services/ingest-gateway/README.md](../../services/ingest-gateway/README.md) · [services/recorder/README.md](../../services/recorder/README.md)
