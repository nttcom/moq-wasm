# [XS] Install prettier from the workspace root in the create-pull-request skill

Next: in `.agents/skills/create-pull-request/SKILL.md:48`, replace `npm --prefix examples/browser ci` with `npm ci`.

## Goal

The lint step installs the prettier the lock file pins. Today the command fails with `EUSAGE`, because the only `package-lock.json` is the workspace root's, and `npx` then downloads an unpinned prettier.
