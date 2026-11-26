# Changelog

## 0.1.0

- The first release of `gst-plugin-moqt`: one archive per platform for
  macOS (Apple silicon, Homebrew's GStreamer), Linux (x86_64 and aarch64,
  glibc 2.35 and GStreamer 1.20 or newer) and Windows (x64, the official
  MSVC GStreamer). Each build is checked to load in `gst-inspect-1.0`.

- The `moqtsink` element publishes H.264 video and AAC audio from a
  GStreamer pipeline to a MoQT relay as an MSF broadcast (LOC and CMAF
  tracks).
