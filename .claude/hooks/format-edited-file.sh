#!/usr/bin/env bash
# PostToolUse hook for Edit and Write: formats the file Claude just wrote with
# the formatter CI checks it with. Edits made through Bash are not seen here;
# the create-pull-request skill runs the full lint before pushing.
set -u

file=$(jq -r '.tool_input.file_path // empty')
[ -n "$file" ] && [ -f "$file" ] || exit 0

root="${CLAUDE_PROJECT_DIR:-$(git -C "$(dirname "$file")" rev-parse --show-toplevel)}"
rel="${file#"$root"/}"

case "$rel" in
  *.rs)
    # rustfmt needs the crate's edition, which differs across the workspace.
    dir=$(dirname "$file")
    while [ "$dir" != "$root" ] && [ ! -f "$dir/Cargo.toml" ]; do dir=$(dirname "$dir"); done
    edition=$(sed -n 's/^edition *= *"\([0-9]*\)"/\1/p' "$dir/Cargo.toml" | head -n 1)
    rustfmt --edition "${edition:-2024}" "$file" >&2 || true
    ;;
  examples/browser/*.js | examples/browser/*.jsx | examples/browser/*.ts | examples/browser/*.tsx | \
  examples/browser/*.json | examples/browser/*.css | examples/browser/*.md | \
  crates/wasm/js/*.js | crates/wasm/js/*.ts | crates/wasm/js/*.json | crates/wasm/js/*.css | crates/wasm/js/*.md)
    prettier="$root/examples/browser/node_modules/.bin/prettier"
    [ -x "$prettier" ] || exit 0
    (cd "$root" && "$prettier" --write --log-level warn \
      --ignore-path .gitignore --ignore-path examples/browser/.prettierignore "$rel") >&2 || true
    ;;
esac

exit 0
