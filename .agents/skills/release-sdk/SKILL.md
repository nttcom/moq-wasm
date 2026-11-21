---
name: release-sdk
description: Release the browser SDK (moq-wasm). Use when the user asks to release the SDK or cut a new moq-wasm version. Collects the リリースノート sections of the PRs merged since the last js-sdk-v* tag into a CHANGELOG.md section, picks the version, opens the release PR, and after it is merged tags it so the SDK release workflow publishes the GitHub Release.
---

# Release the SDK

The release flow itself is described in `docs/architecture/wasm/browser-sdk.md` ("配布").

## Steps

1. `git fetch origin --tags` and take the last release: `git tag --list 'js-sdk-v*' --sort=-v:refname | head -1`.
2. List the PRs merged since then that change what the SDK is built from:
   `git log --format='%s' <tag>..origin/master -- sdk/js crates/moqt crates/broadcast crates/msf crates/mediapack crates/telemetry crates/telemetry-report`.
   Each squash-merged subject ends in `(#<number>)`.
3. Read each PR's body with `gh pr view <number> --json title,body` and take its `## リリースノート` section.
4. Show the user the PRs without that section whose entry is not already under `## Unreleased` in `sdk/js/js/CHANGELOG.md`, with their titles, and ask which of them a user of the SDK would notice. Write an entry for those the user names, following the `## リリースノート` rule of the `create-pull-request` skill.
5. Pick the version: a minor bump (`0.x` → `0.(x+1).0`) when an entry starts with `Breaking:`, a patch bump otherwise. Tell the user which and why.
6. On a branch `chore/wasm-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `sdk/js/js/CHANGELOG.md` with the entries (see *Writing the Section*). Fold in any entries left under `## Unreleased` and remove that heading.
   - Set the version in `sdk/js/js/package.json` and in the `sdk/js/js` entry of `package-lock.json`.
   - Check `node sdk/js/js/scripts/release-notes.mjs sdk/js/js/CHANGELOG.md <version>` prints the section, and run prettier on the changed files.
   - Commit as `chore(wasm): release moq-wasm <version>`.
7. Open the release PR with the `create-pull-request` skill, titled `chore(wasm): moq-wasm <version> をリリースする`. Its body lists the PRs the release contains and has no `## リリースノート`, `## リリースノート（Python）` or `## リリースノート（Rust）` section.
8. After the user merges it, confirm with them, then tag the merge commit and push the tag: `git tag js-sdk-v<version> <merge commit>` and `git push origin js-sdk-v<version>`. Check the commit's `sdk/js/js/package.json` names `<version>` first; the workflow stops otherwise.
9. Wait for the `SDK release` workflow (`gh run list --workflow sdk-release.yml`), then report the GitHub Release URL and its `moq-wasm-<version>.tgz`.

## Writing the Section

The section is read by someone deciding what to change in their page, not by a reviewer of each PR. Edit the PRs' entries instead of pasting them:

- Order the entries: `Breaking:` ones first, then new APIs and behaviour a page can use, then fixed bugs a page could have hit.
- Merge into one closing entry the fixes that ask nothing of a page and that it would rarely hit: protocol conformance (limits, malformed or unknown messages), GOAWAY migration and other edge cases, log and span details. Name what they cover in a sentence or two, with their PR numbers.
- Merge entries about the same API or feature into one.
- Cut each entry to what a page sees or does: drop the cause, the background and the earlier implementation, unless the old behaviour is what a page has to migrate from. Keep the code of a `Breaking:` migration.

Show the user the section before committing it when it merges or cuts PR entries.
