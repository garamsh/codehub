#!/bin/sh
# Runs the workspace tests, sums every "running N tests" line, and fails on a zero total.
# Extra arguments pass through to cargo test (a filter, for instance).
set -eu

selector="--workspace${*:+ $*}"
out=$(mktemp)
trap 'rm -f "$out"' EXIT

status=0
cargo test --workspace --locked "$@" >"$out" 2>&1 || status=$?
cat "$out"

if [ "$status" -ne 0 ]; then
    echo "test: cargo test exited $status" >&2
    exit "$status"
fi

total=$(awk '/^running [0-9]+ tests?$/ { n += $2 } END { print n + 0 }' "$out")
echo "test: $total tests ran ($selector)"
if [ "$total" -eq 0 ]; then
    echo "test: no tests ran ($selector)" >&2
    exit 1
fi
