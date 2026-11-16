#!/usr/bin/env bash
# Fault runs of the codehub chart in a kind cluster (ADR 0057): builds the cluster, installs
# CloudNativePG, distributed MinIO, Dex and the chart, and runs the five scenarios with the load of
# crates/loadtest. Each scenario passes where no acknowledged landing is lost and landings succeed
# again within its stated time. The steps are straight-line; every wait is a `kubectl wait`.
#
# Needs kind, kubectl, helm, docker, openssl, aws and crane, and the images codehub/server:dev
# (`make images`) and the loadtest binary (`cargo build -p loadtest`). KEEP_CLUSTER=1 leaves the
# cluster up. SCENARIOS lists the ones to run, all five by default.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
cluster=${CLUSTER:-codehub-faults}
work=$(mktemp -d)
loadtest=${LOADTEST:-${CARGO_TARGET_DIR:-$root/target}/debug/loadtest}
scenarios=${SCENARIOS:-"api-kill failover pooler registry restore"}
cnpg_version=1.30.0
ns=codehub

cleanup() {
	if [ "${KEEP_CLUSTER:-}" != 1 ]; then kind delete cluster --name "$cluster" || true; fi
	rm -rf "$work"
}
trap cleanup EXIT

k() { kubectl --context "kind-$cluster" -n "$ns" "$@"; }
h() { helm --kube-context "kind-$cluster" -n "$ns" "$@"; }
say() { printf '\n== %s\n' "$*"; }

# The fixture credentials of the check services; the rest are made here and never printed.
. "$root/dev/test.env"

say "cluster"
kind create cluster --name "$cluster" --config "$here/kind.yaml" --wait 120s
kubectl --context "kind-$cluster" create namespace "$ns"
kind load docker-image codehub/server:dev --name "$cluster"
node=$(docker inspect --format '{{ .NetworkSettings.Networks.kind.IPAddress }}' "$cluster-control-plane")
s3_endpoint="http://$node:30900"
issuer="http://$node:30556/dex"
server="http://$node:30080"

say "secrets"
openssl rand -hex 16 | tr -d '\n' >"$work/minio-password"
openssl rand -hex 16 | tr -d '\n' >"$work/pg-password"
openssl rand -hex 32 | tr -d '\n' >"$work/registry-http"
k create secret generic minio-credentials --from-literal=rootUser=codehub --from-file=rootPassword="$work/minio-password"
k create secret generic registry-http --from-file=http-secret="$work/registry-http"
k create secret generic pg-app-user --type=kubernetes.io/basic-auth --from-literal=username=codehub --from-file=password="$work/pg-password"
pg_password=$(cat "$work/pg-password")
printf 'postgres://codehub:%s@pg-rw:5432/codehub' "$pg_password" >"$work/url-direct"
printf 'postgres://codehub:%s@pg-pooler:5432/codehub' "$pg_password" >"$work/url-pooler"
printf 'postgres://codehub:%s@pg-restored-rw:5432/codehub' "$pg_password" >"$work/url-restored"
k create secret generic pg-url-direct --from-file=url="$work/url-direct"
k create secret generic pg-url-pooler --from-file=url="$work/url-pooler"
k create secret generic pg-url-restored --from-file=url="$work/url-restored"
# The registry's signing key (ES256, PKCS#8) and the JWKS it is trusted by. #284's key writer
# replaces these steps.
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$work/signing.pem"
openssl pkey -in "$work/signing.pem" -pubout -outform DER | tail -c 64 >"$work/xy"
x=$(head -c 32 "$work/xy" | basenc --base64url | tr -d '=')
y=$(tail -c 32 "$work/xy" | basenc --base64url | tr -d '=')
printf '{"keys":[{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"kind-key","x":"%s","y":"%s"}]}' "$x" "$y" >"$work/jwks.json"
k create secret generic registry-key --from-file=signing.pem="$work/signing.pem" --from-file=jwks.json="$work/jwks.json"

say "operators and services"
kubectl --context "kind-$cluster" apply --server-side -f "https://github.com/cloudnative-pg/cloudnative-pg/releases/download/v$cnpg_version/cnpg-$cnpg_version.yaml"
kubectl --context "kind-$cluster" -n cnpg-system wait --for=condition=Available deployment/cnpg-controller-manager --timeout=180s
k apply -f "$here/minio.yaml"
k rollout status statefulset/minio --timeout=300s
k wait --for=condition=Complete job/minio-buckets --timeout=300s
sed "s|^issuer:.*|issuer: $issuer|" "$root/dev/dex/config.yaml" >"$work/dex-config.yaml"
k create configmap dex-config --from-file=config.yaml="$work/dex-config.yaml"
k apply -f "$here/dex.yaml"
k wait --for=condition=Available deployment/dex --timeout=120s
k apply -f "$here/postgres.yaml"
k wait --for=condition=Ready cluster/pg --timeout=10m
k wait --for=condition=Available deployment/pg-pooler --timeout=120s

install() { # the chart, with the extra --set arguments given
	h upgrade --install codehub "$root/deploy/helm/codehub" -f "$here/codehub-values.yaml" \
		--set "s3.endpoint=$s3_endpoint" --set "registry.storage.endpoint=$s3_endpoint" \
		--set "registry.realm=$server/v1/registry/token" \
		--set "config.CODEHUB_OIDC_ISSUER=$issuer" "$@" --wait --timeout 10m
}

# load LABEL SECONDS [loadtest arguments]: lands under LABEL for SECONDS, reads every landing back.
load() {
	label=$1 seconds=$2
	shift 2
	LOADTEST_CLIENT_SECRET=$CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET LOADTEST_PASSWORD=$CODEHUB_TEST_OIDC_PASSWORD_ALICE \
		"$loadtest" --server "$server" --token-url "$issuer/token" --client-id "$CODEHUB_TEST_OIDC_TEST_CLIENT_ID" \
		--scope "openid audience:server:client_id:codehub" --agents 8 --duration "$seconds" \
		--label "$label" --record "$work/$label.jsonl" "$@"
}
primary() { k get cluster "$1" -o jsonpath='{.status.currentPrimary}'; }

has() { case " $scenarios " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

say "install"
install --set "postgres.urlSecret.name=pg-url-direct"
load warmup 5

if has api-kill; then
	say "scenario 1: an API pod killed under load"
	load api-kill 60 --fault-after 20 --recover-within 30 \
		--fault "kubectl --context kind-$cluster -n $ns delete \$(kubectl --context kind-$cluster -n $ns get pods -l app.kubernetes.io/component=api -o name | head -n 1) --grace-period=0 --force"
fi

if has failover; then
	say "scenario 2: the PostgreSQL primary killed under load, direct"
	before=$(primary pg)
	load failover 100 --fault-after 20 --recover-within 60 \
		--fault "kubectl --context kind-$cluster -n $ns delete pod $before --grace-period=0 --force"
	k wait --for=condition=Ready cluster/pg --timeout=5m
	after=$(primary pg)
	echo "primary: $before -> $after"
	test "$before" != "$after"
fi

if has pooler; then
	say "scenario 3: the primary killed under load, through the pooler in transaction mode"
	mode=$(k get pooler pg-pooler -o jsonpath='{.spec.pgbouncer.poolMode}')
	echo "pooler mode: $mode"
	test "$mode" = transaction
	install --set "postgres.urlSecret.name=pg-url-pooler" --set "postgres.statementCache=0"
	before=$(primary pg)
	load pooler 120 --fault-after 20 --recover-within 75 \
		--fault "kubectl --context kind-$cluster -n $ns delete pod $before --grace-period=0 --force"
	k wait --for=condition=Ready cluster/pg --timeout=5m
	after=$(primary pg)
	echo "primary: $before -> $after"
	test "$before" != "$after"
fi

if has registry; then
	say "scenario 4: the registry at 3 replicas, pushes and pulls spread across them"
	k scale deployment/codehub-registry --replicas=3
	k rollout status deployment/codehub-registry --timeout=120s
	token=$(LOADTEST_CLIENT_SECRET=$CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET LOADTEST_PASSWORD=$CODEHUB_TEST_OIDC_PASSWORD_ALICE \
		"$loadtest" --server "$server" --token-url "$issuer/token" --client-id "$CODEHUB_TEST_OIDC_TEST_CLIENT_ID" \
		--scope "openid audience:server:client_id:codehub" --print-token)
	export DOCKER_CONFIG="$work/docker"
	mkdir -p "$DOCKER_CONFIG"
	printf '%s' "$token" | crane auth login "$node:30500" -u alice --password-stdin --insecure
	for n in $(seq 1 30); do
		crane copy --insecure gcr.io/distroless/static:nonroot@sha256:aafe993665bf6200db38a02c11a9351e782a84b6a45e7f33b6695638c88e3931 "$node:30500/acme/app/spread-$n:latest" >/dev/null
		crane digest --insecure "$node:30500/acme/app/spread-$n:latest" >/dev/null
	done
	for pod in $(k get pods -l app.kubernetes.io/component=registry -o name); do
		served=$(k logs "$pod" | grep -c '/v2/' || true)
		echo "$pod served $served registry requests"
		test "$served" -gt 0
	done
fi

if has restore; then
	say "scenario 5: restore drill"
	install --set "postgres.urlSecret.name=pg-url-direct"
	load before-backup 30
	k apply -f "$here/backup.yaml"
	k wait --for=jsonpath='{.status.phase}'=completed backup/pg-drill --timeout=10m
	export AWS_ACCESS_KEY_ID=codehub AWS_SECRET_ACCESS_KEY=$(cat "$work/minio-password") AWS_DEFAULT_REGION=us-east-1
	# The objects are copied after the database backup, so they are a superset of what it refers to.
	aws --endpoint-url "$s3_endpoint" s3 sync s3://codehub s3://codehub-backup --only-show-errors
	load after-backup 30
	k apply -f "$here/postgres-restore.yaml"
	k wait --for=condition=Ready cluster/pg-restored --timeout=10m
	aws --endpoint-url "$s3_endpoint" s3 sync s3://codehub-backup s3://codehub-restored --only-show-errors
	install --set "postgres.urlSecret.name=pg-url-restored" --set "postgres.migrateUrlSecret.name=pg-url-restored" \
		--set "s3.bucket=codehub-restored"
	say "restored: what landed before the backup must be there and intact"
	verify() {
		LOADTEST_CLIENT_SECRET=$CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET LOADTEST_PASSWORD=$CODEHUB_TEST_OIDC_PASSWORD_ALICE \
			"$loadtest" --server "$server" --token-url "$issuer/token" --client-id "$CODEHUB_TEST_OIDC_TEST_CLIENT_ID" \
			--scope "openid audience:server:client_id:codehub" "$@"
	}
	verify --verify "$work/before-backup.jsonl"
	say "restored: what landed after it may be absent, but no landing refers to a missing object"
	verify --verify "$work/after-backup.jsonl" --tolerate-missing
fi

say "done"
