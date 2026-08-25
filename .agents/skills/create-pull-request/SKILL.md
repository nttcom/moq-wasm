---
name: create-pull-request
description: Create a pull request for the current branch. Use when the user asks to open, create, or update a PR. Writes the title and body in Japanese, fills in the repository PR template, and adds a mermaid diagram when it makes the change easier to review.
---

# Create Pull Request

## Steps

1. Review the change: `git status`, `git diff origin/master...HEAD`, and `git log origin/master..HEAD --oneline`.
2. Commit any uncommitted work. Commit messages are English and follow the Conventional Commits rules in `AGENTS.md`.
3. Push the branch: `git push -u origin <branch>`.
4. Create the PR with `gh pr create --base master --title <title> --body <body>`.
   - If a PR already exists for the branch, push the new commits and update the existing PR with `gh pr edit` instead of creating a duplicate.
5. Report the PR URL.

## Title

- Write in Japanese.
- Prefix with the Conventional Commits type and scope, matching the commits: `feat(relay): ...`.
- State what changed, not how it was implemented.

## Body

Use `.github/pull_request_template.md` as-is. Keep every heading, replace each HTML comment with the actual content, and delete no section — write `なし` when a section does not apply.

Section rules:

- `## 概要` — exactly 3 lines. One line each for background, what was done, and the result. This is the only part some reviewers read.
- `## 関連タスク` — link issues as `#<IssueNumber>`. This repository is public OSS, so never paste internal tracker URLs.
- `## やらないこと` — state what was deliberately left out, so reviewers do not look for it.
- `## 影響範囲` — name the affected crates and whether the change is breaking for dependents. `moqt` is the central crate, so changes there affect the whole workspace.
- `## テスト` — the commands actually run and their results. If a check was skipped, say so.

Write the whole body in Japanese.

## Mermaid Diagrams

Add a mermaid diagram only when prose alone is hard to follow — a changed message flow between client, relay, and server, a state transition, or a new module dependency. Place it under `## やったこと`.

Skip the diagram for changes that are already clear in text, such as documentation edits, dependency bumps, or single-function fixes. A diagram that just restates the file list is noise.

```mermaid
sequenceDiagram
    Client->>Relay: SUBSCRIBE
    Relay->>Publisher: SUBSCRIBE
    Publisher-->>Relay: SUBSCRIBE_OK
    Relay-->>Client: SUBSCRIBE_OK
```
