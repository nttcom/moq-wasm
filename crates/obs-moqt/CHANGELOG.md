# Changelog

## 0.1.0

- The first release of `obs-moqt`, as an unsigned installer for OBS Studio
  32.0 on macOS (Apple silicon).

- **Start Streaming** publishes OBS's H.264 video and AAC audio to a MoQT
  relay as an MSF broadcast (LOC and CMAF tracks).

- A **MoQT** dock holds the relay URL, the namespace, and the token or the
  path of a token file. A token file is read again every 10 s while
  streaming, so a stream can outlast its first token.
