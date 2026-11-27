---
name: release-rust-sdk
description: Release the Rust SDK (moq-wasm, sdk/rust). Collects リリースノート（Rust） sections since the last rust-sdk-v* tag into sdk/rust/CHANGELOG.md, picks the version, opens the release PR, and tags after merge to test and publish the GitHub Release.
---

# Release the Rust SDK (`sdk/rust`)

Reference: `docs/architecture/moq-wasm/sdk.md` ("Release").

## Steps

1. Fetch tags and get the latest release:
   `git fetch origin --tags && git tag --list 'rust-sdk-v*' --sort=-v:refname | head -1`
   If no tag exists (first release), use the version in `sdk/rust/Cargo.toml` and `## Unreleased` entries, skipping steps 2–5.
2. List merged PRs touching crate inputs since `<tag>`:
   `git log --format='%s' <tag>..origin/master -- sdk/rust crates/moqt crates/broadcast crates/msf crates/mediapack crates/auth-token crates/robotics`
   (Squash-merged subjects end in `(#<number>)`.)
3. Read each PR's title and body (prefer PR read tool; fallback: `gh pr view <number> --json title,body`) and collect `## リリースノート（Rust）`.
4. Show the user PRs missing that section whose entry is not already under `## Unreleased` in `sdk/rust/CHANGELOG.md`; ask which are user-visible and write entries per `create-pull-request` skill rules.
5. Pick version: minor bump (`0.x` → `0.(x+1).0`) if any entry starts with `Breaking:`, else patch bump. Tell the user the version and reason.
6. Create branch `chore/rust-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `sdk/rust/CHANGELOG.md` following `## Writing the Section` in `.agents/skills/release-sdk/SKILL.md` ("page" → "application"); fold in and remove `## Unreleased`.
   - Set `[package].version` in `sdk/rust/Cargo.toml`, run `cargo check -p moq-wasm` to update `Cargo.lock`, and verify the lock diff only changes `moq-wasm`.
   - Update install snippets in `sdk/rust/README.md` and `docs/architecture/moq-wasm/sdk.md` to `rust-sdk-v<version>`.
   - Verify `node sdk/js/js/scripts/release-notes.mjs sdk/rust/CHANGELOG.md <version>` prints the section.
   - Commit: `chore(moq-wasm): release moq-wasm (Rust) <version>`.
7. Open the release PR via `create-pull-request`:
   - Title: `chore(moq-wasm): moq-wasm (Rust) <version> をリリースする`
   - Body: list included PRs; omit all `## リリースノート*` sections.
8. After the user merges, confirm with them, verify `<merge commit>`'s `sdk/rust/Cargo.toml` has `<version>` (workflow fails otherwise), then tag and push:
   `git tag rust-sdk-v<version> <merge commit> && git push origin rust-sdk-v<version>`
9. Wait for `Rust SDK release` (`gh run list --workflow rust-sdk-release.yml`), then report the GitHub Release URL. If tests fail against newer unlocked dependencies, report the failing crate—fix it in a separate release rather than re-running.
