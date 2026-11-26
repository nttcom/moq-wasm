# gst-plugin-moqt Architecture

`moqtsink` is a GStreamer element with `video` and `audio` request pads that publishes through `broadcast::BroadcastWriter`. Why it subclasses `gst::Element`: [gstreamer.md](gstreamer.md). Element behavior: [README](../../../crates/gst-plugin-moqt/README.md).

## Release
A release is a GitHub Release with one archive per platform, each holding the plugin library and `LICENSE`.

| Platform | Archive | Built on | Loads into |
| --- | --- | --- | --- |
| macOS (Apple silicon) | `.tar.gz` | `macos-15`, Homebrew's `gstreamer` | Homebrew's GStreamer |
| Linux (x86_64, aarch64) | `.tar.gz` | `ubuntu:22.04` container (glibc 2.35, GStreamer 1.20) | Any GStreamer 1.20+ on glibc 2.35+ |
| Windows (x64) | `.zip` | `windows-2025`, official MSVC GStreamer (checksum-pinned) | The official MSVC GStreamer |

1. A release PR bumps `crates/gst-plugin-moqt/Cargo.toml` and adds the version's section to `crates/gst-plugin-moqt/CHANGELOG.md`.
2. After merge, the tag `gst-plugin-moqt-v<version>` is pushed on that commit.
3. `.github/workflows/gst-plugin-release.yml` checks the tag against the version and the changelog and calls `.github/workflows/gst-plugin.yml`, which also runs on pull requests that touch the crate.
4. It creates the GitHub Release with the archives and the changelog section (not marked Latest).

- **Checks in the build**: Each job extracts its archive into GStreamer's user plugin directory, as the README tells users to, and requires `gst-inspect-1.0 moqtsink` to succeed. Nothing is published.
- **Linking**: The library links GStreamer and GLib dynamically and everything else (Rust code, aws-lc) statically, so it loads into a GStreamer whose ABI it shares. On macOS that ABI includes the install names of Homebrew's libraries, so `GStreamer.framework` cannot load it.
- **Unsigned**: The macOS library has only the linker's ad-hoc signature. The README installs it with `curl`, which sets no quarantine attribute; a browser download needs `xattr -d com.apple.quarantine`.
