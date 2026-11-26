# `cargo-about` for the License List of a Package (`obs-moqt`)

**Status:** Accepted (2026-11-26)

## What

Use the `cargo-about` command (0.9.3, a checksum-pinned release binary in CI) to generate `THIRD-PARTY-LICENSES.txt`, the licenses of the libraries built into the plugin, which every package ships. It is a build tool: no crate depends on it.

## Context

A package holds the plugin as a binary with about 150 crates built in, most under MIT, Apache-2.0, ISC, BSD-3-Clause, or Unicode-3.0. Those licenses ask that their text and copyright notices accompany a copy, which a binary does not do by itself. The plugin is GPL-3.0-or-later, so each library's license must also be compatible with it.

## Alternatives

- **A list kept by hand:** Goes stale with every dependency update, and nothing fails when it does.
- **A script over `cargo metadata`:** Has to find each crate's license files, pick a side of every `OR` expression, and supply the text for crates that ship no license file, which is what `cargo-about` does.
- **`cargo-deny`:** Checks licenses against an allow-list but writes no list of their texts.

## Decision

Use `cargo-about` with `crates/obs-moqt/about.toml` and `about.hbs`. One run gives the list and the check: it fails on a library whose license is not in `accepted`. The list covers the three release targets in one file, so one CI job makes it for every package. It does not cover the Rust standard library, which `COPYRIGHT` names instead.
