#!/bin/sh
# Pulls the garam images dev/garam/compose.yaml names, by the references written there, into the
# local Docker image store, where compose finds them under pull_policy: never. The registry login
# is the caller's. Fails unless the file names exactly two.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

refs=$(sed -n 's/^ *image: \(.*\/garam\/\(server\|policy\):[^ ]*@sha256:[0-9a-f]*\)$/\1/p' "$here/compose.yaml")
test "$(printf '%s\n' "$refs" | wc -l)" -eq 2
for ref in $refs; do
    docker pull "$ref"
done
