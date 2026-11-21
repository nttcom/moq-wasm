---
name: release-python-sdk
description: Release the Python SDK (moq-wasm, sdk/python). Use when the user asks to release the Python SDK or cut a new version of its wheels. Collects the リリースノート（Python） sections of the PRs merged since the last python-sdk-v* tag into a CHANGELOG.md section, picks the version, opens the release PR, and after it is merged tags it so the Python SDK release workflow publishes the GitHub Release with the wheels.
---

# Release the Python SDK

The release flow itself is described in `docs/architecture/python/sdk.md` ("Package").

## Steps

1. `git fetch origin --tags` and take the last release: `git tag --list 'python-sdk-v*' --sort=-v:refname | head -1`. With no tag yet, this is the first release: it takes the version already in `sdk/python/pyproject.toml` and the entries under `## Unreleased`, so skip steps 2 to 5.
2. List the PRs merged since then that change what the wheel is built from:
   `git log --format='%s' <tag>..origin/master -- sdk/python crates/moqt crates/broadcast crates/msf crates/mediapack crates/telemetry crates/telemetry-report`.
   Each squash-merged subject ends in `(#<number>)`.
3. Read each PR's body with `gh pr view <number> --json title,body` and take its `## リリースノート（Python）` section.
4. Show the user the PRs without that section whose entry is not already under `## Unreleased` in `sdk/python/CHANGELOG.md`, with their titles, and ask which of them a user of the Python SDK would notice. Write an entry for those the user names, following the `## リリースノート（Python）` rule of the `create-pull-request` skill.
5. Pick the version: a minor bump (`0.x` → `0.(x+1).0`) when an entry starts with `Breaking:`, a patch bump otherwise. Tell the user which and why.
6. On a branch `chore/python-release-<version>` from `origin/master`:
   - Add `## <version>` under `# Changelog` in `sdk/python/CHANGELOG.md` with the entries, `Breaking:` ones first. Fold in any entries left under `## Unreleased` and remove that heading.
   - Set `version` under `[project]` in `sdk/python/pyproject.toml`. maturin takes the wheel's version from there; the `version` in `sdk/python/Cargo.toml` is not read.
   - Run `uv lock` in every directory whose `uv.lock` names the package (`git grep -l 'name = "moq-wasm"' -- '*uv.lock'`), and check the diff only changes the `moq-wasm` version. The workflow's `uv sync --locked` fails on a stale `sdk/python/uv.lock`.
   - Check `node sdk/js/js/scripts/release-notes.mjs sdk/python/CHANGELOG.md <version>` prints the section.
   - Commit as `chore(python): release moq-wasm (Python) <version>`.
7. Open the release PR with the `create-pull-request` skill, titled `chore(python): moq-wasm (Python) <version> をリリースする`. Its body lists the PRs the release contains and has no `## リリースノート`, `## リリースノート（Python）` or `## リリースノート（Rust）` section.
8. After the user merges it, confirm with them, then tag the merge commit and push the tag: `git tag python-sdk-v<version> <merge commit>` and `git push origin python-sdk-v<version>`. Check the commit's `sdk/python/pyproject.toml` names `<version>` first; the workflow stops otherwise.
9. Wait for the `Python SDK release` workflow (`gh run list --workflow python-release.yml`), then report the GitHub Release URL and its three wheels (`moq_wasm-<version>-cp311-abi3-{manylinux_2_28_x86_64,manylinux_2_28_aarch64,macosx_11_0_arm64}.whl`).
