#!/bin/sh
# Brings up the garam stack (dev/garam/compose.yaml) with fresh throwaway keys, runs the end-to-end
# test against it (e2e/tests/garam.rs) and removes the stack, its volumes and the keys at every
# exit, whatever the test's result. Run from the repository root by `make e2e-garam`.
# Arguments pass to `cargo test` (`-- --ignored` runs the ignored tests).
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

set -a
. "$here/env.sh"
set +a

dir=$(mktemp -d)
CODEHUB_GARAM_E2E_DIR=$dir
CODEHUB_GARAM_E2E_USER=$(id -u):$(id -g)
CODEHUB_GARAM_E2E_COMPOSE=$here/compose.yaml
export CODEHUB_GARAM_E2E_DIR CODEHUB_GARAM_E2E_USER CODEHUB_GARAM_E2E_COMPOSE

compose() { docker compose -f "$CODEHUB_GARAM_E2E_COMPOSE" "$@"; }

cleanup() {
    status=$?
    compose down --volumes --remove-orphans >/dev/null 2>&1 || true
    rm -rf "$dir"
    exit "$status"
}
trap cleanup EXIT INT TERM

sh "$here/keys.sh" "$dir"

# The policy service trusts the keys the running issuer publishes, so the api and the gateway
# start first and the policy service after the keys are fetched.
compose up --detach --wait gateway machine
curl --fail --silent --show-error --retry 10 --retry-all-errors --retry-delay 1 --cacert "$dir/ca.crt" \
    "$CODEHUB_GARAM_E2E_ISSUER/oauth/jwks" >"$dir/issuer-jwks.json"
compose up --detach --wait policy
compose run --rm -T --no-deps api policy-feed start

# Pointed at by the test: SSL_CERT_FILE is what lets codehub trust the certificate authority.
CODEHUB_GARAM_E2E_CA=$dir/ca.crt
export CODEHUB_GARAM_E2E_CA

cd "$root"
TEST_SELECTOR="-p e2e --test garam" sh scripts/test-count.sh "$@"
