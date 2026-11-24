# transcode

In-process GStreamer H.264 ladder transcoder (`Transcoder`) and offline multi-participant MP4 compositor (`Compositor`) operating on `mediapack::MediaEvent`s.

## Transcoding (live H.264 ABR ladder)

Re-encodes an H.264 Annex-B video stream into lower-resolution renditions with aligned keyframes (`draft-ietf-moq-cmsf-01` §3.2). Audio is never transcoded — forward audio events as-is.

```
VideoSample (Annex-B) ─> appsrc ─> h264parse ─> decodebin ─> videoconvert ─> tee
                                                   ├─> videoscale ─> x264enc ─> appsink ─> rendition 0
                                                   └─> videoscale ─> x264enc ─> appsink ─> rendition 1
```

```rust
use transcode::{Transcoder, ladder_for};

let renditions = ladder_for(source_sps.width, source_sps.height); // e.g. 720p/480p/360p for 1080p; empty for ≤360p
if renditions.is_empty() {
    return Ok(()); // Transcoder::new requires at least one rendition
}
let mut transcoder = Transcoder::new(&renditions)?;
let input = transcoder.input(); // cloneable handle for pushing from another thread/task
for sample in &video_samples {
    input.push(sample)?;
}
input.finish()?;
while let Some(output) = transcoder.next().await {
    let output = output?;                      // output.rendition indexes `renditions`
    publish(output.rendition, output.event);   // VideoConfig once, then Video samples
}
```

- `ladder_for` never upscales (returns standard heights `1080`/`720`/`480`/`360` strictly below source height).
- Run the bundled CLI example on the test fixture:

```shell
cargo run -p transcode --example transcode -- crates/mediapack/fixtures/testsrc.ts out 54:100
```

## Compositing (offline multi-participant MP4)

`Compositor` mixes multiple participant recordings into one progressive MP4 (1280×720 @ 15 fps H.264 2 Mbps + stereo AAC 128 kbps): screen share or active speaker large on the left, other participants in a right-hand strip.

```rust
use transcode::{CompositeInput, Compositor, Loudness};

let loudness = Loudness::measure(audio_events_again)?; // blocking decode pass
let inputs = vec![CompositeInput {
    video: Some(video_events),
    screen_share: None,
    audio: Some(audio_events),
    loudness: Some(loudness),
}];
let mut compositor = Compositor::start(inputs, &output_path)?;
compositor.finished().await?; // dropping cancels the job
```

- Each `EventSource` yields a config event followed by samples on a shared timeline; audio-only inputs join only the audio mix.
- Active speaker switches from 100 ms RMS `Loudness` (loudest above ~−40 dBFS after 1 s of speech); video stalled for >1 s leaves the stage until it resumes.
- Supported inputs: H.264, H.265, VP8, VP9, AV1 video; AAC, Opus, A-law, MP3 audio.

## Requirements

GStreamer 1.20+ (`brew install gstreamer` on macOS installs build headers and all plugins below; on Debian/Ubuntu install `pkg-config`, `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, plus the plugin packages below):

| Package | Transcoder | Compositor adds |
| --- | --- | --- |
| `gst-plugins-base` | `appsrc`/`appsink`, `decodebin`, `videoscale` | `compositor`, `audiomixer`, `opusdec` |
| `gst-plugins-good` | | `isomp4` (`mp4mux`), `vpx`, `alaw`, `mpg123`, `audioparsers` |
| `gst-plugins-bad` | `h264parse` | `av1dec` (or `dav1ddec` via `gst-plugin-dav1d`), `av1parse`, `opusparse` |
| `gst-plugins-ugly` | `x264enc` | |
| `gst-libav` | decoder (or platform decoder) | `avdec_h264`, `avenc_aac` |
