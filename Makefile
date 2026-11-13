.PHONY: lint format test e2e e2e-garam e2e-gaon loadtest loadtest-smoke build images ci services services-down

lint:
	CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --locked

format:
	cargo fmt --all --check

# Starts the check services (compose.yaml), smoke-tests them, and runs the
# workspace tests, without the e2e package, with the CODEHUB_TEST_* variables
# of dev/check-env.sh.
# The services stay up afterwards; `make services-down` stops them.
test: services
	set -a && . dev/check-env.sh && set +a && sh scripts/test-count.sh

# Builds the binaries the e2e tests run, then runs the e2e package against the same services
# and variables as `test`.
e2e: services
	cargo build --locked -p codehub -p ch -p codehub-runner
	set -a && . dev/check-env.sh && set +a && TEST_SELECTOR="-p e2e" sh scripts/test-count.sh -- --skip garam_

# The e2e tests whose names contain garam_ run against garam's published images (dev/garam), which
# the garam-e2e workflow alone may pull, so `e2e` skips them and this target runs them: it starts
# the garam stack, runs them and removes the stack and its keys at every exit. Needs the images in
# the local Docker image store; it is not part of `ci`.
e2e-garam: services
	cargo build --locked -p codehub -p ch
	set -a && . dev/check-env.sh && set +a && sh dev/garam/run.sh

# The live tests of the gaon driver (crates/codehub-runner/tests/gaon_live.rs) against a gaon-apiserver
# built from the commit pinned in dev/gaon/env.sh and run with etcd under RBAC: it builds the server
# in a container, starts the stack, runs the tests and removes the stack at every exit. It needs
# Docker and the two public images pinned there, and network access to fetch gaon; it is not part
# of `ci`.
e2e-gaon:
	sh dev/gaon/run.sh

# The load harness against two codehub replicas and the check services: a smoke run of every
# scenario in the debug build, which fails on an unexpected status or a replica that dies.
loadtest-smoke: services
	cargo build --locked -p codehub -p loadtest
	set -a && . dev/check-env.sh && set +a && cargo run --locked -p loadtest -- --scenario all --agents 4 --duration 5

# The full run for the scale baseline: release binaries, 200 agents, LOADTEST_DURATION seconds per
# scenario; the figures are also written to target/loadtest.json.
LOADTEST_DURATION ?= 60

loadtest: services
	cargo build --locked --release -p codehub -p loadtest
	set -a && . dev/check-env.sh && set +a && cargo run --locked --release -p loadtest -- --scenario all --agents 200 --duration $(LOADTEST_DURATION) --json target/loadtest.json

build:
	cargo build --workspace --all-targets --locked

# Builds the server and runner images (deploy/), then runs each image's binary with --help and
# checks that it runs as a user other than root. IMAGE_VERSION and IMAGE_REVISION are the
# image's version and revision labels.
IMAGE_VERSION ?= dev
IMAGE_REVISION ?= $(shell git rev-parse HEAD)

images:
	set -eu; \
	for name in server runner; do \
		image=codehub/$$name:$(IMAGE_VERSION); \
		docker build -f deploy/$$name.Dockerfile \
			--build-arg IMAGE_VERSION=$(IMAGE_VERSION) --build-arg IMAGE_REVISION=$(IMAGE_REVISION) \
			-t $$image .; \
		docker run --rm $$image --help; \
		uid=$$(docker run --rm --entrypoint id $$image -u); \
		size=$$(docker image inspect --format '{{.Size}}' $$image); \
		echo "images: $$image runs as uid $$uid, $$size bytes"; \
		test "$$uid" != 0; \
	done

ci: lint format test e2e loadtest-smoke build

services:
	sh scripts/services-up.sh
	sh scripts/check-services.sh

services-down:
	docker compose down --volumes
