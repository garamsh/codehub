#!/bin/sh
# Builds gaon-apiserver from garamsh/gaon at the commit pinned in env.sh, starts it with etcd under
# RBAC and service-account tokens, applies the runner's ClusterRole and binding, runs the live tests
# of the gaon driver (crates/codehub-runner/tests/gaon_live.rs) against it, and removes the
# containers, the process and the temporary directory at every exit, whatever the test's result.
# Run from the repository root by `make e2e-gaon`. Arguments pass to `cargo test`, after the test
# selection. The built binary stays in target/codehub-gaon-e2e/, keyed by the commit, so a second
# run skips the build; `cargo clean` removes it. Every wait is bounded, so a stalled service fails
# the run instead of hanging it. Only public images are pulled.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

set -a
. "$here/env.sh"
set +a

cache=$root/target/codehub-gaon-e2e
dir=$(mktemp -d -t codehub-gaon-e2e.XXXXXX)
etcd=codehub-gaon-e2e-etcd
build=codehub-gaon-e2e-build
pid=

cleanup() {
    status=$?
    if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    docker rm --force --volumes "$etcd" "$build" >/dev/null 2>&1 || true
    rm -rf "$dir"
    exit "$status"
}
trap cleanup EXIT INT TERM

# Every openssl and curl call is bounded, so a stalled one fails the run.
openssl() { timeout 30 openssl "$@"; }
admin_curl() {
    timeout 60 curl --fail --silent --show-error --max-time 10 --cacert "$dir/ca.crt" \
        --cert "$dir/admin.crt" --key "$dir/admin.key" "$@"
}

mkdir -p "$cache"
binary=$cache/gaon-apiserver-$CODEHUB_GAON_E2E_SHA
if [ ! -x "$binary" ]; then
    mkdir "$dir/src"
    (
        cd "$dir/src"
        git init --quiet
        timeout 300 git fetch --quiet --depth 1 "$CODEHUB_GAON_E2E_REPOSITORY" "$CODEHUB_GAON_E2E_SHA"
        git checkout --quiet FETCH_HEAD
    )
    test "$(git -C "$dir/src" rev-parse HEAD)" = "$CODEHUB_GAON_E2E_SHA"
    mkdir -p "$cache/go"
    # The module's replace of lib/api is a relative path, so the whole clone is mounted.
    timeout 900 docker run --rm --name "$build" --user "$(id -u):$(id -g)" \
        --volume "$dir/src:/src" --volume "$cache/go:/go-cache" --volume "$dir:/out" \
        --workdir /src/apiserver \
        --env HOME=/tmp --env GOCACHE=/go-cache/build --env GOMODCACHE=/go-cache/mod \
        --env GOTOOLCHAIN=local --env CGO_ENABLED=0 --env GOFLAGS=-buildvcs=false \
        "$CODEHUB_GAON_E2E_GO_IMAGE" go build -o /out/gaon-apiserver ./cmd/gaon-apiserver
    mv "$dir/gaon-apiserver" "$binary"
fi

# The certificate authority, the server certificate for localhost, the administrator's client
# certificate (group gaon:admins) and the service-account signing key (P-256).
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 \
    -subj "/CN=codehub-gaon-e2e-ca" -keyout "$dir/ca.key" -out "$dir/ca.crt" 2>/dev/null
signed() {
    name=$1
    subject=$2
    usage=$3
    san=$4
    openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj "$subject" \
        -keyout "$dir/$name.key" -out "$dir/$name.csr" 2>/dev/null
    printf 'extendedKeyUsage=%s\nsubjectAltName=%s\n' "$usage" "$san" >"$dir/$name.ext"
    openssl x509 -req -in "$dir/$name.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" -CAcreateserial \
        -days 2 -extfile "$dir/$name.ext" -out "$dir/$name.crt" 2>/dev/null
}
signed server "/CN=localhost" serverAuth "DNS:localhost,IP:127.0.0.1"
signed admin "/O=gaon:admins/CN=codehub-gaon-e2e-admin" clientAuth "DNS:localhost"
openssl ecparam -name prime256v1 -genkey -noout -out "$dir/signing.key" 2>/dev/null
cat "$dir/admin.key" "$dir/admin.crt" >"$dir/admin.pem"

docker run --detach --name "$etcd" --publish "127.0.0.1:$CODEHUB_GAON_E2E_ETCD_PORT:2379" \
    "$CODEHUB_GAON_E2E_ETCD_IMAGE" etcd --data-dir /etcd-data \
    --listen-client-urls http://0.0.0.0:2379 \
    --advertise-client-urls "http://127.0.0.1:$CODEHUB_GAON_E2E_ETCD_PORT" >/dev/null
ready=
for _ in $(seq 1 30); do
    if timeout 10 docker exec "$etcd" etcdctl endpoint health >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 1
done
test -n "$ready" || { echo "e2e-gaon: etcd did not become healthy" >&2; exit 1; }

"$binary" --listen "127.0.0.1:$CODEHUB_GAON_E2E_API_PORT" \
    --etcd-endpoints "http://127.0.0.1:$CODEHUB_GAON_E2E_ETCD_PORT" \
    --authorization-mode RBAC \
    --tls-client-ca "$dir/ca.crt" --tls-cert "$dir/server.crt" --tls-key "$dir/server.key" \
    --service-account-issuer "$CODEHUB_GAON_E2E_ISSUER" \
    --service-account-signing-key-file "$dir/signing.key" >"$dir/apiserver.log" 2>&1 &
pid=$!

rbac=https://localhost:$CODEHUB_GAON_E2E_API_PORT/apis/rbac.gaon.garam.sh/v1alpha1
# The server bootstraps its roles at start, so the administrator's first list succeeding is the
# readiness of RBAC as well as of the listener.
ready=
for _ in $(seq 1 60); do
    if admin_curl "$rbac/clusterroles" >/dev/null 2>&1; then
        ready=1
        break
    fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
done
if [ -z "$ready" ]; then
    echo "e2e-gaon: gaon-apiserver did not serve; its log:" >&2
    tail -n 40 "$dir/apiserver.log" >&2
    exit 1
fi

admin_curl --request PUT --header 'content-type: application/json' \
    --data @"$here/runner-role.json" "$rbac/clusterroles/codehub-runner" >/dev/null
admin_curl --request PUT --header 'content-type: application/json' \
    --data @"$here/runner-binding.json" "$rbac/clusterrolebindings/codehub-runner" >/dev/null

CODEHUB_GAON_E2E_CA=$dir/ca.crt
CODEHUB_GAON_E2E_ADMIN_PEM=$dir/admin.pem
CODEHUB_GAON_E2E_DIR=$dir
export CODEHUB_GAON_E2E_CA CODEHUB_GAON_E2E_ADMIN_PEM CODEHUB_GAON_E2E_DIR

echo "e2e-gaon: gaon-apiserver built from garamsh/gaon $CODEHUB_GAON_E2E_SHA, etcd $CODEHUB_GAON_E2E_ETCD_IMAGE"
cd "$root"
status=0
TEST_SELECTOR="-p codehub-runner --test gaon_live" sh scripts/test-count.sh -- --ignored "$@" || status=$?
if [ "$status" -ne 0 ]; then
    echo "e2e-gaon: gaon-apiserver log tail:" >&2
    tail -n 40 "$dir/apiserver.log" >&2
fi
exit "$status"
