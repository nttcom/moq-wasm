---
name: release-sdk
description: Release the browser SDK (moq-wasm). Collects リリースノート sections since the last js-sdk-v* tag into sdk/js/js/CHANGELOG.md, picks the version, opens the release PR, and tags after merge to publish the GitHub Release.
---

# Release the Browser SDK (`moq-wasm`)

Reference: `docs/architecture/wasm/browser-sdk.md` ("配布").

## Steps

1. Fetch tags and get the latest release:
   `git fetch origin --tags && git tag --list 'js-sdk-v*' --sort=-v:refname | head -1`
2. List merged PRs touching SDK inputs since `<tag>`:
   `git log --format='%s' <tag>..origin/master -- sdk/js crates/moqt crates/broadcast crates/msf crates/mediapack crates/telemetry crates/telemetry-report`
   (Squash-merged subjects end in `(#<number>)`.)
3. Read each PR's title and body (prefer PR read tool; fallback: `gh pr view <number> --json title,body`) and collect `## リリースノート`.
4. Show the user PRs missing `## リリースノート` whose entry is not already under `## Unreleased` in `sdk/js/js/CHANGELOG.md`; ask which are user-visible and write entries per `create-pull-request` skill rules.
5. Pick version: minor bump (`0.x` → `0.(x+1).0`) if any entry starts with `Breaking:`, else patch bump. Tell the user the version and reason.
6. Create branch `chore/wasm-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `sdk/js/js/CHANGELOG.md` (*Writing the Section*); fold in and remove `## Unreleased`.
   - Set `<version>` in `sdk/js/js/package.json` and the `sdk/js/js` entry in `package-lock.json`.
   - Verify `node sdk/js/js/scripts/release-notes.mjs sdk/js/js/CHANGELOG.md <version>` prints the section; run prettier on changed files.
   - Commit: `chore(wasm): release moq-wasm <version>`.
7. Open the release PR via `create-pull-request`:
   - Title: `chore(wasm): moq-wasm <version> をリリースする`
   - Body: list included PRs; omit all `## リリースノート*` sections.
8. After the user merges, confirm with them, verify `<merge commit>`'s `sdk/js/js/package.json` has `<version>` (workflow fails otherwise), then tag and push:
   `git tag js-sdk-v<version> <merge commit> && git push origin js-sdk-v<version>`
9. Wait for `SDK release` (`gh run list --workflow sdk-release.yml`), then report the GitHub Release URL and `moq-wasm-<version>.tgz`.

## Writing the Section

Write for a page author deciding what to change, not a PR reviewer—edit entries rather than pasting verbatim:
- Order: `Breaking:` first → new APIs/behavior → user-facing bug fixes.
- Group low-impact fixes (protocol limits/malformed messages, GOAWAY migration/edge cases, log/span details) into one closing bullet summarizing coverage with PR numbers.
- Merge entries touching the same API or feature.
- Keep only what a page sees or does; drop cause, background, and old implementation unless needed for migration (keep `Breaking:` migration code).
- Show the section to the user before committing if entries were merged or cut.
