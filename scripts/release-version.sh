#!/bin/sh
# Prints the image version a release tag names: the tag `vX.Y.Z`, `vX.Y.Z-dev.N` or `vX.Y.Z-rc.N`
# gives the tag without its `v`. N is a positive number with no leading zero. Any other tag
# fails, so that nothing is published under a name the immutable repositories cannot take back.
# Used by .github/workflows/release.yml, which passes the tag as its one argument.
set -eu

tag=${1:?release-version: no tag given}
newline='
'
case $tag in
*"$newline"*) valid=no ;;
*) valid=yes ;;
esac
if [ "$valid" = no ] || ! printf '%s' "$tag" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(dev|rc)\.[1-9][0-9]*)?$'; then
    echo "release-version: tag is not vX.Y.Z, vX.Y.Z-dev.N or vX.Y.Z-rc.N: nothing published" >&2
    exit 1
fi
printf '%s\n' "${tag#v}"
