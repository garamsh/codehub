#!/usr/bin/env bash
# Fault runs of the codehub chart in a kind cluster (ADR 0057): builds the cluster, installs
# CloudNativePG, distributed MinIO, Dex and the chart, and runs the five scenarios with the load of
# crates/loadtest. Each scenario passes where no acknowledged landing is lost and landings succeed
# again within its stated time. The steps are straight-line; every wait is a `kubectl wait`.
#
# Needs kind, kubectl, helm, docker, openssl, aws, crane and cargo, and the images codehub/server:dev
# (`make images`) and the loadtest binary (`cargo build -p loadtest`); a run also reaches the
# network (ADR 0057). It refuses to start where a cluster of its name exists, and deletes only the
# one it made. KEEP_CLUSTER=1 leaves the cluster up. SCENARIOS lists the ones to run, all five by
# default. BREAK_RESTORED=1 deletes an object from the restored bucket and expects the restore
# drill to fail on it, which is how the drill's own verdict is checked. Where a run fails, the pods' logs and the events are printed before the cluster goes.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
cluster=${CLUSTER:-codehub-faults}
work=$(mktemp -d)
loadtest=${LOADTEST:-${CARGO_TARGET_DIR:-$root/target}/debug/loadtest}
scenarios=${SCENARIOS:-"api-kill failover pooler registry restore"}
cnpg_version=1.30.0
ns=codehub

if kind get clusters 2>/dev/null | grep -qx "$cluster"; then
	echo "a kind cluster named $cluster exists; delete it, or set CLUSTER to another name" >&2
	exit 1
fi
created=
evidence() { # what a failed run leaves to be read: events, pods, and the logs of every pod
	local namespace pod
	kubectl --context "kind-$cluster" get events -A --sort-by=.lastTimestamp || true
	kubectl --context "kind-$cluster" get pods -A -o wide || true
	for namespace in "$ns" cnpg-system; do
		for pod in $(kubectl --context "kind-$cluster" -n "$namespace" get pods -o name 2>/dev/null); do
			printf '\n--- logs of %s/%s\n' "$namespace" "$pod"
			kubectl --context "kind-$cluster" -n "$namespace" logs "$pod" --all-containers --tail=200 || true
		done
	done
}
cleanup() {
	local status=$?
	if [ -n "$created" ]; then
		if [ "$status" != 0 ]; then evidence; fi
		if [ "${KEEP_CLUSTER:-}" != 1 ]; then kind delete cluster --name "$cluster" || true; fi
	fi
	rm -rf "$work"
}
trap cleanup EXIT

k() { kubectl --context "kind-$cluster" -n "$ns" "$@"; }
h() { helm --kube-context "kind-$cluster" -n "$ns" "$@"; }
say() { printf '\n== %s\n' "$*"; }

# The fixture credentials of the check services; the rest are made here and never printed.
. "$root/dev/test.env"

say "cluster"
created=1
kind create cluster --name "$cluster" --config "$here/kind.yaml" --wait 120s
kubectl --context "kind-$cluster" create namespace "$ns"
kind load docker-image codehub/server:dev --name "$cluster"
node=$(docker inspect --format '{{ .NetworkSettings.Networks.kind.IPAddress }}' "$cluster-control-plane")
s3_endpoint="http://$node:30900"
issuer="http://$node:30556/dex"
server="http://$node:30080"
# crane refuses a token realm that is an address in a private range, so the realm names the node by
# a name that resolves to it (nip.io); the pushes themselves go to the address.
realm="http://${node//./-}.nip.io:30080/v1/registry/token"

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
# The registry's signing key (ES256, PKCS#8), its id and the JWKS the registry trusts it by are the
# check environment's own (the e2e example garam_keys, which `make e2e` uses).
mkdir "$work/keys"
(cd "$root" && cargo run --locked -q -p e2e --example garam_keys -- "$work/keys")
registry_kid=$(cat "$work/keys/registry.kid")
k create secret generic registry-key --from-file=signing.pem="$work/keys/registry.pem" --from-file=jwks.json="$work/keys/registry-jwks.json"

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
		--set "registry.realm=$realm" --set "registry.keyId=$registry_kid" \
		--set "config.CODEHUB_OIDC_ISSUER=$issuer" "$@" --wait --timeout 10m
}

# load LABEL SECONDS [loadtest arguments]: lands under LABEL for SECONDS, reads every landing back.
load() {
	local label=$1 seconds=$2
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
	install --set "postgres.urlSecret.name=pg-url-pooler"
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
	install --set "postgres.urlSecret.name=pg-url-direct" --set registry.replicas=3
	k rollout status deployment/codehub-registry --timeout=120s
	token=$(LOADTEST_CLIENT_SECRET=$CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET LOADTEST_PASSWORD=$CODEHUB_TEST_OIDC_PASSWORD_ALICE \
		"$loadtest" --server "$server" --token-url "$issuer/token" --client-id "$CODEHUB_TEST_OIDC_TEST_CLIENT_ID" \
		--scope "openid audience:server:client_id:codehub" --print-token)
	export DOCKER_CONFIG="$work/docker"
	mkdir -p "$DOCKER_CONFIG"
	printf '%s' "$token" | crane auth login "$node:30500" -u alice --password-stdin --insecure
	base=gcr.io/distroless/static:nonroot@sha256:aafe993665bf6200db38a02c11a9351e782a84b6a45e7f33b6695638c88e3931
	# Each image has a layer of its own, so each push uploads a blob no other has and a pull reads
	# it back from the bucket through whichever replica the connection reaches.
	for n in $(seq 1 30); do
		mkdir "$work/layer-$n"
		head -c 65536 /dev/urandom >"$work/layer-$n/blob"
		tar -cf "$work/layer-$n.tar" -C "$work/layer-$n" blob
		crane append --insecure -b "$base" -f "$work/layer-$n.tar" -t "$node:30500/acme/app/spread-$n:latest" >/dev/null
	done
	for n in $(seq 1 30); do
		crane pull --insecure "$node:30500/acme/app/spread-$n:latest" "$work/pulled-$n.tar"
	done
	# Requests are counted from the access log by what they are and that they succeeded: a blob
	# upload that completed (a PUT answered 201) and a blob read (a GET answered 307, the redirect to the bucket that serves the bytes). The probes,
	# the challenges (401) and the manifests are not counted.
	for pod in $(k get pods -l app.kubernetes.io/component=registry -o name); do
		uploads=$(k logs "$pod" | grep -cE '"PUT /v2/[^ ]*/blobs/uploads/[^ ]* HTTP/1.1" 201' || true)
		reads=$(k logs "$pod" | grep -cE '"GET /v2/[^ ]*/blobs/sha256:[^ ]* HTTP/1.1" 307' || true)
		echo "$pod completed $uploads blob uploads and redirected $reads blob reads to the bucket"
		test "$uploads" -gt 0
		test "$reads" -gt 0
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
	backup_id=$(k get backup/pg-drill -o jsonpath='{.status.backupId}')
	sed "s/BACKUP_ID/$backup_id/" "$here/postgres-restore.yaml" | k apply -f -
	# The full recovery replays the WAL of every earlier scenario from the bucket; it took 9 minutes in a run, and two replicas follow it.
	k wait --for=condition=Ready cluster/pg-restored --timeout=25m
	aws --endpoint-url "$s3_endpoint" s3 sync s3://codehub-backup s3://codehub-restored --only-show-errors
	install --set "postgres.urlSecret.name=pg-url-restored" --set "postgres.migrateUrlSecret.name=pg-url-restored" \
		--set "s3.bucket=codehub-restored"
	verify() {
		LOADTEST_CLIENT_SECRET=$CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET LOADTEST_PASSWORD=$CODEHUB_TEST_OIDC_PASSWORD_ALICE \
			"$loadtest" --server "$server" --token-url "$issuer/token" --client-id "$CODEHUB_TEST_OIDC_TEST_CLIENT_ID" \
			--scope "openid audience:server:client_id:codehub" "$@"
	}
	if [ "${BREAK_RESTORED:-}" = 1 ]; then
		# A check of the drill: one object of a landing the backup holds is deleted from the restored
		# bucket, and the drill must fail on it even where a missing landing is tolerated.
		say "restored, with one object deleted: the drill must fail"
		digest=$(head -n 1 "$work/before-backup.jsonl" | jq -j .content | sha256sum | cut -d' ' -f1)
		key=$(aws --endpoint-url "$s3_endpoint" s3 ls s3://codehub-restored --recursive | awk -v d="$digest" '$4 ~ d "$" {print $4; exit}')
		test -n "$key"
		aws --endpoint-url "$s3_endpoint" s3 rm "s3://codehub-restored/$key"
		if verify --verify "$work/before-backup.jsonl" --tolerate-missing; then
			echo "the drill passed with an object deleted from the restored bucket" >&2
			exit 1
		fi
		echo "the drill failed on the deleted object, as it must"
		exit 0
	fi
	say "restored: what landed before the backup must be there and intact"
	verify --verify "$work/before-backup.jsonl"
	say "restored: what landed after it may be absent, but no landing refers to a missing object"
	verify --verify "$work/after-backup.jsonl" --tolerate-missing
fi

say "done"
