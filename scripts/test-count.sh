#!/bin/sh
# Runs the tests of one selection of packages, sums every "running N tests" line, and fails on a
# zero total. TEST_SELECTOR names the packages (default: the workspace without the e2e package);
# extra arguments pass through to cargo test (a filter, for instance).
set -eu

packages=${TEST_SELECTOR:---workspace --exclude e2e}
selector="$packages${*:+ $*}"
out=$(mktemp)
trap 'rm -f "$out"' EXIT

status=0
cargo test $packages --locked "$@" >"$out" 2>&1 || status=$?
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
