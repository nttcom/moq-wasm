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
(`faststart`), re-encoded as H.264 on a black background: a screen share, or else the active
speaker's video, large on the left, the other videos in a strip on the right. Their audio is
mixed into one stereo AAC track.

```
EventSource ─> appsrc ─> decodebin3 ─> videoconvert ───────────────> compositor ─> x264enc ─┐
EventSource ─> appsrc ─> decodebin3 ─> audioconvert ─> audioresample ─> audiomixer ─> avenc_aac ─┴─> mp4mux ─> file
```

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

Each `EventSource` yields a config event, then samples already on the shared timeline. The
stage follows the participants whose video covers each output frame: someone alone fills the
picture, and a participant whose video stops for more than a second leaves the stage until
it resumes. An input without video only joins the audio mix. A source that yields no sample,
such as a recorded track that never received one, is treated as `None`.

`Loudness::measure` decodes a participant's audio before compositing and keeps its RMS level
per 100 ms. From these the compositor picks the active speaker over the whole recording: the
loudest participant above about -40 dBFS, who takes over after 1 s of speech, with pauses
under 1.5 s kept in one turn. Because the whole recording is known in advance, a switch lands
where the new speaker starts talking, and the first speaker holds the stage from the start.

A screen share takes the focus while it has frames, each lasting up to 5 s since browsers send
frames only when the screen changes; a share started later waits in the strip until the first
ends. Without a share, the speaker's camera takes the focus; while the speaker shows no video,
the focus stays where it was. The strip holds the speaker's camera first, then the other
videos in input order, 1/5 of the output wide, its tiles shrinking to fit the height.

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
