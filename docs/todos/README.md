# TODOs

Add a TODO with `/todo-add`; do one with `/todo-start`.
One file is one TODO, done in one PR, then deleted; git history keeps it.

## Priority

- `high/` — wrong behaviour: a bug, a draft MUST violation, a security hole, or something that blocks other work.
- `medium/` — a suspected bug, or a shape `.agents/rules/coding-style.md` says to fix.
- `low/` — cleanup, or a design idea no task needs yet.

When unsure, pick the lower one.

## Format

`<priority>/<scope>-<slug>.md`, e.g. `high/relay-retry-forwarded-publish.md`.

```markdown
# [S] Imperative title

Next: the first action, with a `path:line`.

## Goal

The observable outcome, with its evidence.

## Plan

Optional: settled decisions with reasons, and open questions.
```

Size the title `[XS]` to `[XL]`. Keep each section to three lines.
