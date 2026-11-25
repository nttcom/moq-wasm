---
name: release-python-sdk
description: Release the Python SDK (moq-wasm, sdk/python). Collects リリースノート（Python） sections since the last python-sdk-v* tag into sdk/python/CHANGELOG.md, picks the version, opens the release PR, and tags after merge to publish the wheels.
---

# Release the Python SDK (`sdk/python`)

Reference: `docs/architecture/python/sdk.md` ("Package").

## Steps

1. Fetch tags and get the latest release:
   `git fetch origin --tags && git tag --list 'python-sdk-v*' --sort=-v:refname | head -1`
   If no tag exists (first release), use the version in `sdk/python/pyproject.toml` and `## Unreleased` entries, skipping steps 2–5.
2. List merged PRs touching wheel inputs since `<tag>`:
   `git log --format='%s' <tag>..origin/master -- sdk/python crates/moqt crates/broadcast crates/msf crates/mediapack crates/telemetry crates/telemetry-report`
   (Squash-merged subjects end in `(#<number>)`.)
3. Read each PR's title and body (prefer PR read tool; fallback: `gh pr view <number> --json title,body`) and collect `## リリースノート（Python）`.
4. Show the user PRs missing that section whose entry is not already under `## Unreleased` in `sdk/python/CHANGELOG.md`; ask which are user-visible and write entries per `create-pull-request` skill rules.
5. Pick version: minor bump (`0.x` → `0.(x+1).0`) if any entry starts with `Breaking:`, else patch bump. Tell the user the version and reason.
6. Create branch `chore/python-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `sdk/python/CHANGELOG.md` following `## Writing the Section` in `.agents/skills/release-sdk/SKILL.md` ("page" → "application"); fold in and remove `## Unreleased`.
   - Set `[project].version` in `sdk/python/pyproject.toml` (maturin reads this, not `sdk/python/Cargo.toml`).
   - Run `uv lock` in every directory whose `uv.lock` lists `moq-wasm` (`git grep -l 'name = "moq-wasm"' -- '*uv.lock'`) and verify the diff only updates `moq-wasm` (CI's `uv sync --locked` fails if stale).
   - Verify `node sdk/js/js/scripts/release-notes.mjs sdk/python/CHANGELOG.md <version>` prints the section.
   - Commit: `chore(python): release moq-wasm (Python) <version>`.
7. Open the release PR via `create-pull-request`:
   - Title: `chore(python): moq-wasm (Python) <version> をリリースする`
   - Body: list included PRs; omit all `## リリースノート*` sections.
8. After the user merges, confirm with them, verify `<merge commit>`'s `sdk/python/pyproject.toml` has `<version>` (workflow fails otherwise), then tag and push:
   `git tag python-sdk-v<version> <merge commit> && git push origin python-sdk-v<version>`
9. Wait for `Python SDK release` (`gh run list --workflow python-release.yml`), then report the GitHub Release URL and its three wheels (`moq_wasm-<version>-cp311-abi3-{manylinux_2_28_x86_64,manylinux_2_28_aarch64,macosx_11_0_arm64}.whl`).
