# transcode

Re-encodes H.264 video from mediapack `MediaEvent`s into several renditions with an
in-process GStreamer pipeline. Audio is not touched; callers forward it as-is.

```
VideoSample (Annex-B) ─> appsrc ─> h264parse ─> decodebin ─> videoconvert ─> tee
                                                   ├─> videoscale ─> x264enc ─> appsink ─> rendition 0 events
                                                   └─> videoscale ─> x264enc ─> appsink ─> rendition 1 events
```

## Usage

```rust
use transcode::{Transcoder, ladder_for};

let renditions = ladder_for(source_sps.width, source_sps.height); // e.g. 720p/480p/360p below a 1080p source
let mut transcoder = Transcoder::new(&renditions)?;
transcoder.push(&video_sample)?;              // any number of times
while let Some(output) = transcoder.next().await {
    let output = output?;                      // output.rendition indexes `renditions`
    publish(output.rendition, output.event);   // VideoConfig once, then Video samples
}
```

`ladder_for` never upscales: it returns the standard heights (1080/720/480/360) strictly
below the source height with matching bitrates. Call `finish()` to flush the encoders;
`next()` returns `None` once every rendition has ended.

Each encoder uses `x264enc tune=zerolatency` with a fixed 60-frame keyframe interval
and a baseline profile, so keyframes are not aligned with the source GOP yet.

## Requirements

GStreamer 1.20+ with `gst-plugins-base` (appsrc/appsink, decodebin, videoscale),
`gst-plugins-bad` (h264parse), `gst-plugins-ugly` (x264enc) and `gst-libav` or a
platform decoder. On macOS `brew install gstreamer` installs all of them.

```shell
cargo run -p transcode --example transcode -- ../mediapack/fixtures/testsrc.ts out 54:100
```
