# Changelog

## 0.1.1

- Every package now carries `COPYRIGHT` (the plugin's copyright notice and
  its license, GPL-3.0-or-later) and `THIRD-PARTY-LICENSES.txt` (the licenses
  of the libraries built into the plugin), next to `LICENSE`. The plugin
  itself is unchanged.

## 0.1.0

- The first release of `obs-moqt` for OBS Studio 32: an unsigned installer
  for macOS (Apple silicon), a zip for Windows (x64) and a tarball for Linux
  (x86_64, Ubuntu 24.04 with the `obsproject` PPA). Streaming is tested on
  macOS; the Windows and Linux builds are checked to load in OBS.

- **Start Streaming** publishes OBS's H.264 video and AAC audio to a MoQT
  relay as an MSF broadcast (LOC and CMAF tracks).

- A **MoQT** dock holds the relay URL, the namespace, and the token or the
  path of a token file. A token file is read again every 10 s while
  streaming, so a stream can outlast its first token.
