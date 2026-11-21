---
name: release-rust-sdk
description: Release the Rust SDK (moq-wasm, sdk/rust). Use when the user asks to release the Rust SDK or cut a new version of the crate. Collects the リリースノート（Rust） sections of the PRs merged since the last rust-sdk-v* tag into a CHANGELOG.md section, picks the version, opens the release PR, and after it is merged tags it so the Rust SDK release workflow tests the crate and publishes the GitHub Release.
---

# Release the Rust SDK

The release flow itself is described in `docs/architecture/moq-wasm/sdk.md` ("Release"). Users depend on the tag, so the release has no build artifacts.

## Steps

1. `git fetch origin --tags` and take the last release: `git tag --list 'rust-sdk-v*' --sort=-v:refname | head -1`. With no tag yet, this is the first release: it takes the version already in `sdk/rust/Cargo.toml` and the entries under `## Unreleased`, so skip steps 2 to 5.
2. List the PRs merged since then that change what the crate is built from:
   `git log --format='%s' <tag>..origin/master -- sdk/rust crates/moqt crates/broadcast crates/msf crates/mediapack crates/auth-token`.
   Each squash-merged subject ends in `(#<number>)`.
3. Read each PR's body with `gh pr view <number> --json title,body` and take its `## リリースノート（Rust）` section.
4. Show the user the PRs without that section whose entry is not already under `## Unreleased` in `sdk/rust/CHANGELOG.md`, with their titles, and ask which of them a user of the Rust SDK would notice. Write an entry for those the user names, following the `## リリースノート（Rust）` rule of the `create-pull-request` skill.
5. Pick the version: a minor bump (`0.x` → `0.(x+1).0`) when an entry starts with `Breaking:`, a patch bump otherwise. Tell the user which and why.
6. On a branch `chore/rust-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `sdk/rust/CHANGELOG.md` with the entries, `Breaking:` ones first. Fold in any entries left under `## Unreleased` and remove that heading.
   - Set `version` under `[package]` in `sdk/rust/Cargo.toml`, run `cargo check -p moq-wasm` so `Cargo.lock` follows, and check the lock's diff only changes the `moq-wasm` version.
   - Replace the previous tag in the install snippet of `sdk/rust/README.md` with `rust-sdk-v<version>`.
   - Check `node sdk/js/js/scripts/release-notes.mjs sdk/rust/CHANGELOG.md <version>` prints the section.
   - Commit as `chore(moq-wasm): release moq-wasm (Rust) <version>`.
7. Open the release PR with the `create-pull-request` skill, titled `chore(moq-wasm): moq-wasm (Rust) <version> をリリースする`. Its body lists the PRs the release contains and has no `## リリースノート`, `## リリースノート（Python）` or `## リリースノート（Rust）` section.
8. After the user merges it, confirm with them, then tag the merge commit and push the tag: `git tag rust-sdk-v<version> <merge commit>` and `git push origin rust-sdk-v<version>`. Check the commit's `sdk/rust/Cargo.toml` names `<version>` first; the workflow stops otherwise.
9. Wait for the `Rust SDK release` workflow (`gh run list --workflow rust-sdk-release.yml`), then report the GitHub Release URL. When its tests fail on dependencies newer than the lock file, report the failing crate: users resolve them too, so the fix is a release of its own, not a re-run.
