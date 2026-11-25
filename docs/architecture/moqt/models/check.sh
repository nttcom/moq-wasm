#!/usr/bin/env bash
# Checks every instance of every model exhaustively with TLC and compares the
# outcome of its `safety` invariant with the expectation below.
#
# Usage (Java 17+ on PATH):  docs/architecture/moqt/models/check.sh [file.qnt ...]
# QUINT overrides the quint command (default: npx @informalsystems/quint@0.34.0).
set -uo pipefail
cd "$(dirname "$0")"
trap 'rm -rf _apalache-out' EXIT
QUINT=${QUINT:-"npx --yes @informalsystems/quint@0.34.0"}

# Each instance module is preceded by `// expect: ok` (its `safety` holds) or
# `// expect: violation` (it models a known defect, or turns off a guard to
# show the invariant is not vacuous).

files=("$@")
[ ${#files[@]} -eq 0 ] && files=(*.qnt)
failed=0
for file in "${files[@]}"; do
  while read -r want main; do
    out=$($QUINT verify --backend tlc --main "$main" --invariant safety "$file" 2>&1)
    got=$(grep -oE '^\[(ok|violation)\]' <<<"$out" | head -1 | tr -d '[]')
    states=$(grep -oE '[0-9,]+ distinct states' <<<"$out" | head -1)
    if [[ $got == "$want" ]]; then mark=PASS; else mark=FAIL; failed=1; fi
    printf '%s %-55s expected %-9s got %-9s %s\n' "$mark" "$main" "$want" "${got:-error}" "$states"
  done < <(awk '/^\/\/ expect: / { want = $3 } /^module / && want { print want, $2; want = "" }' "$file")
done
exit $failed
