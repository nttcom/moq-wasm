# `cc` to Compile the Dock (`obs-moqt`)

**Status:** Accepted (2026-11-25)

## What

Add `cc` as a build-dependency of `obs-moqt`. `build.rs` uses it to compile `src/dock.cpp` (C++17) into the plugin when `OBS_MOQT_QT_CFLAGS` is set.

## Context

OBS's **Settings → Stream** page does not list services that plugins register, so the plugin needs a dock of its own for its settings. OBS takes a dock as a Qt `QWidget`, which only C++ can build, while the rest of the plugin is Rust.

## Alternatives

- **Run the compiler from `build.rs` by hand:** Re-implements what `cc` does (finding the compiler, target and optimization flags, archiving, linking the C++ runtime) for each platform.
- **CMake and a separate dock library:** A second build system and a second file for OBS to load, for one source file.
- **Qt bindings for Rust (`cxx-qt`, `qmetaobject`):** A larger dependency and a code generator, for a form with five fields.

## Decision

Use `cc`. It is already in the workspace's dependency graph (`aws-lc-sys` builds with it), so nothing new is downloaded, and it stays out of the plugin binary.
