---
name: release-sdk
description: Release the browser SDK (moq-wasm). Use when the user asks to release the SDK or cut a new moq-wasm version. Collects the リリースノート sections of the PRs merged since the last sdk-v* tag into a CHANGELOG.md section, picks the version, opens the release PR, and after it is merged tags it so the SDK release workflow publishes the GitHub Release.
---

# Release the SDK

The release flow itself is described in `docs/architecture/wasm/browser-sdk.md` ("配布"). PRs do not edit `crates/wasm/js/CHANGELOG.md`; each one that a user of the SDK can notice carries its entry in a `## リリースノート` section of its body (see the `create-pull-request` skill).

## Steps

1. `git fetch origin --tags` and take the last release: `git tag --list 'sdk-v*' --sort=-v:refname | head -1`.
2. List the PRs merged since then that change what the SDK is built from:
   `git log --format='%s' <tag>..origin/master -- crates/wasm crates/moqt crates/broadcast crates/msf crates/mediapack crates/client-telemetry`.
   Each squash-merged subject ends in `(#<number>)`.
3. Read each PR's body with `gh pr view <number> --json title,body` and take its `## リリースノート` section.
4. Show the user the PRs without that section, with their titles, and ask which of them a user of the SDK would notice. Write an entry for those the user names, in the same style.
5. Pick the version: a minor bump (`0.x` → `0.(x+1).0`) when an entry starts with `Breaking:`, a patch bump otherwise. Tell the user which and why.
6. On a branch `chore/wasm-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `crates/wasm/js/CHANGELOG.md` with the entries, `Breaking:` ones first. Fold in any entries left under `## Unreleased` and remove that heading.
   - Set the version in `crates/wasm/js/package.json` and in the `crates/wasm/js` entry of `package-lock.json`.
   - Check `node crates/wasm/js/scripts/release-notes.mjs <version>` prints the section, and run prettier on the changed files.
   - Commit as `chore(wasm): release moq-wasm <version>`.
7. Open the release PR with the `create-pull-request` skill, titled `chore(wasm): moq-wasm <version> をリリースする`. Its body lists the PRs the release contains and has no `## リリースノート` section.
8. After the user merges it, confirm with them, then tag the merge commit and push the tag: `git tag sdk-v<version> <merge commit>` and `git push origin sdk-v<version>`. Check the commit's `crates/wasm/js/package.json` names `<version>` first; the workflow stops otherwise.
9. Wait for the `SDK release` workflow (`gh run list --workflow sdk-release.yml`), then report the GitHub Release URL and its `moq-wasm-<version>.tgz`.
