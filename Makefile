# The test credentials, read once here and exported to every recipe and to the compose commands.
include dev/test.env
export

# Extra arguments for cargo nextest run in `test` and `e2e`: a filter, for instance.
TEST_ARGS ?=

.PHONY: lint format test e2e e2e-garam e2e-gaon loadtest loadtest-smoke build images ci services services-down

lint:
	CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --locked

format:
	cargo fmt --all --check

# Starts the check services (compose.yaml) and runs the workspace tests, without the e2e
# package, with the CODEHUB_TEST_* variables of dev/test.env. A run that selects no test fails.
# The services stay up afterwards; `make services-down` stops them.
test: services
	cargo nextest run --workspace --exclude e2e --locked --no-tests=fail $(TEST_ARGS)
	cargo test --doc --workspace --locked

# Runs the e2e package, against the same services and variables as `test`, on the binaries `build`
# produced: a build selecting fewer packages resolves other feature sets and compiles the shared
# dependencies again.
e2e: build services
	cargo nextest run -p e2e --locked --no-tests=fail -E 'not test(garam_)' $(TEST_ARGS)

# The e2e tests whose names contain garam_ run against garam's published images, the garam profile
# of compose.yaml, which the garam-e2e workflow alone may pull, so `e2e` skips them and this target
# runs them: it generates throwaway keys, brings the profile up with `up --wait` as a project of
# its own, runs them, and removes the project, its volumes and the keys at every exit. Needs the
# images in the local Docker image store; it is not part of `ci`.
# The one shell line is make's missing "always run" step: a trap that removes the stack on any exit,
# the test's failure and an interrupt included.
e2e-garam: export CODEHUB_GARAM_E2E_DIR := $(CURDIR)/target/garam-e2e
e2e-garam: export CODEHUB_GARAM_E2E_CA := $(CURDIR)/target/garam-e2e/ca.crt
e2e-garam: export CODEHUB_GARAM_E2E_COMPOSE := $(CURDIR)/compose.yaml
e2e-garam: export CODEHUB_GARAM_E2E_USER := $(shell id -u):$(shell id -g)
e2e-garam: services
	cargo build --locked -p codehub -p ch
	trap 'docker compose --env-file dev/test.env -p $(CODEHUB_GARAM_E2E_PROJECT) --profile garam down --volumes --remove-orphans; rm -rf "$$CODEHUB_GARAM_E2E_DIR"' EXIT INT TERM; \
	rm -rf "$$CODEHUB_GARAM_E2E_DIR" && mkdir -p "$$CODEHUB_GARAM_E2E_DIR" && cargo run --locked -p e2e --example garam_keys -- "$$CODEHUB_GARAM_E2E_DIR" && \
	docker compose --env-file dev/test.env -p $(CODEHUB_GARAM_E2E_PROJECT) --profile garam up --detach --wait --wait-timeout 180 gateway && \
	cargo nextest run -p e2e --locked --no-tests=fail --test garam

# The live tests of the gaon driver (crates/codehub-runner/tests/gaon_live.rs) against a gaon-apiserver
# built from the commit pinned in dev/gaon/env.sh and run with etcd under RBAC: it builds the server
# in a container, starts the stack, runs the tests and removes the stack at every exit. It needs
# Docker and the two public images pinned there, and network access to fetch gaon; it is not part
# of `ci`.
e2e-gaon:
	sh dev/gaon/run.sh

# The load harness against two codehub replicas and the check services: a smoke run of every
# scenario in the debug build, which fails on an unexpected status or a replica that dies. The
# codehub binary it starts is the one `build` produced.
loadtest-smoke: build services
	cargo run --locked -p loadtest -- --scenario all --agents 4 --duration 5

# The full run for the scale baseline: release binaries, 200 agents, LOADTEST_DURATION seconds per
# scenario; the figures are also written to target/loadtest.json.
LOADTEST_DURATION ?= 60

loadtest: services
	cargo build --locked --release -p codehub -p loadtest
	cargo run --locked --release -p loadtest -- --scenario all --agents 200 --duration $(LOADTEST_DURATION) --json target/loadtest.json

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

ci: lint format build test e2e loadtest-smoke

# Running containers are kept; pick up a changed image or configuration with
# `make services-down` and then `make services`.
services:
	docker compose --env-file dev/test.env up --detach --wait --wait-timeout 180 --no-recreate

services-down:
	docker compose --env-file dev/test.env down --volumes
