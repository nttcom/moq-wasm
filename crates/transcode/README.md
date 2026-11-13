# transcode

Re-encodes H.264 video from mediapack `MediaEvent`s into several renditions
with an in-process GStreamer pipeline. Audio is not touched; callers forward
it as-is.

```
VideoSample (Annex-B) ─> appsrc ─> h264parse ─> decodebin ─> videoconvert ─> tee
                                                   ├─> videoscale ─> x264enc ─> appsink ─> rendition 0 events
                                                   └─> videoscale ─> x264enc ─> appsink ─> rendition 1 events
```

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

- `ladder_for` never upscales: it returns the standard heights
  (1080/720/480/360) strictly below the source height.
- `input()` returns a cloneable `TranscodeInput`, so a blocking feeder thread
  can push while an async task drains `next()`. `finish()` flushes the
  encoders; `next()` then returns `None`.
- Rendition keyframes land exactly where the source's do
  (draft-ietf-moq-cmsf-01 §3.2).

```shell
cargo run -p transcode --example transcode -- ../mediapack/fixtures/testsrc.ts out 54:100
```

## Compositing

`Compositor` composites several participants' recordings offline into one
progressive MP4: a screen share, or else the active speaker's video, large on
the left, the other videos in a strip on the right, on a black background.
Their audio is mixed into one stereo AAC track.

```rust
use transcode::{CompositeInput, CompositeSettings, Compositor, Loudness};

let loudness = Loudness::measure(audio_events_again)?; // blocks; decodes the audio once more
let inputs = vec![CompositeInput {
    video: Some(video_events),
    screen_share: None,
    audio: Some(audio_events),
    loudness: Some(loudness),
}];
let mut compositor = Compositor::start(inputs, CompositeSettings::default(), &output_path)?;
compositor.finished().await?;                  // dropping the compositor cancels
```

- Each `EventSource` yields a config event, then samples already on the
  shared timeline. An input without video only joins the audio mix.
- The active speaker is picked from `Loudness`, the RMS level per 100 ms: the
  loudest participant above about -40 dBFS, after 1 s of speech.
- A participant whose video stops for more than a second leaves the stage
  until it resumes.
- Inputs may be H.264, VP8, VP9 or AV1 video and AAC, Opus, A-law or MP3
  audio.

## Requirements

GStreamer 1.20+. On macOS `brew install gstreamer` installs every plugin.

| Package | Transcoder | Compositor adds |
| --- | --- | --- |
| `gst-plugins-base` | appsrc/appsink, decodebin, videoscale | `compositor`, `audiomixer`, `opusdec` |
| `gst-plugins-good` | | `isomp4` (`mp4mux`), `vpx`, `alaw`, `mpg123`, `audioparsers` |
| `gst-plugins-bad` | `h264parse` | `dav1d` or `aom` (AV1), `av1parse`, `opusparse` |
| `gst-plugins-ugly` | `x264enc` | |
| `gst-libav` | a decoder (or a platform one) | `avdec_h264`, `avenc_aac` |
