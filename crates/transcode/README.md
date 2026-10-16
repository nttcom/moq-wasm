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

`input()` returns a cloneable `TranscodeInput` with the same `push` / `finish`, so a
blocking feeder thread can own the input while an async task drains `next()`.

`ladder_for` never upscales: it returns the standard heights (1080/720/480/360) strictly
below the source height with matching bitrates. Call `finish()` to flush the encoders;
`next()` returns `None` once every rendition has ended.

Each encoder uses `x264enc tune=zerolatency` with a baseline profile. Its own keyframe
interval is pushed out of reach (`key-int-max=i32::MAX`) and a keyframe is forced on the
frame carrying each source keyframe's presentation time, so rendition keyframes land
exactly where the source's do (draft-ietf-moq-cmsf-01 §3.2).

## Compositing

`Compositor` composites several participants' recordings offline into one progressive MP4
(`faststart`): their videos as a grid of equal tiles on a black background, re-encoded as
H.264, and their audio mixed into one stereo AAC track.

```
EventSource ─> appsrc ─> decodebin3 ─> videoconvert ───────────────> compositor ─> x264enc ─┐
EventSource ─> appsrc ─> decodebin3 ─> audioconvert ─> audioresample ─> audiomixer ─> avenc_aac ─┴─> mp4mux ─> file
```

```rust
use transcode::{CompositeInput, CompositeSettings, Compositor};

let inputs = vec![CompositeInput { video: Some(video_events), audio: Some(audio_events) }];
let mut compositor = Compositor::start(inputs, CompositeSettings::default(), &output_path)?;
compositor.finished().await?;                  // dropping the compositor cancels
```

Each `EventSource` yields a config event, then samples already on the shared timeline. The
grid follows the participants whose video covers each output frame (ceil(sqrt(n)) columns):
someone alone fills the picture, and a participant whose video stops for more than a second
leaves the grid until it resumes. An input without video only joins the audio mix.

Inputs may be H.264, VP8, VP9 or AV1 video and AAC, Opus, A-law or MP3 audio, which needs
these plugins on top of the transcoder's:

| Package | Plugins |
| --- | --- |
| `gst-plugins-base` | `compositor`, `audiomixer`, `opusdec` |
| `gst-plugins-good` | `isomp4` (`mp4mux`), `vpx` (`vp8dec`/`vp9dec`), `alaw`, `mpg123`, `audioparsers` (`aacparse`, `mpegaudioparse`) |
| `gst-plugins-bad` | `dav1d` or `aom` (AV1), `videoparsersbad` (`h264parse`, `av1parse`), `opusparse` |
| `gst-plugins-ugly` | `x264` (`x264enc`) |
| `gst-libav` | `avdec_h264`, `avenc_aac` |

## Requirements

GStreamer 1.20+ with `gst-plugins-base` (appsrc/appsink, decodebin, videoscale),
`gst-plugins-bad` (h264parse), `gst-plugins-ugly` (x264enc) and `gst-libav` or a
platform decoder. On macOS `brew install gstreamer` installs all of them.

```shell
cargo run -p transcode --example transcode -- ../mediapack/fixtures/testsrc.ts out 54:100
```
