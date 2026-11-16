# The test credentials, read once here and exported to every recipe and to the compose commands.
include dev/test.env
export

# Extra arguments for cargo nextest run in `test` and `e2e`: a filter, for instance.
TEST_ARGS ?=

.PHONY: lint format test e2e e2e-garam e2e-registry e2e-gaon loadtest loadtest-smoke build images ci mutants services services-down

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
	cargo nextest run -p e2e --locked --no-tests=fail -E 'not test(garam_) and not test(registry_)' $(TEST_ARGS)

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

# The e2e tests whose names contain registry_ run against the registry of compose.yaml's profile
# `registry` (Distribution v3, ADR 0053), so `e2e` skips them and this target runs them: it
# generates the throwaway keys, adds the registry to the running check services without recreating
# them, runs the tests, and removes the registry, its bucket service and the keys at every exit; the
# registry's bucket stays in the store, which `services-down` removes. Not part of `ci`.
e2e-registry: export CODEHUB_REGISTRY_E2E_DIR := $(CURDIR)/target/registry-e2e
e2e-registry: services
	cargo build --locked -p codehub -p ch
	trap 'docker compose --env-file dev/test.env --profile registry rm --stop --force --volumes registry registry-bucket; rm -rf "$$CODEHUB_REGISTRY_E2E_DIR"' EXIT INT TERM; \
	rm -rf "$$CODEHUB_REGISTRY_E2E_DIR" && mkdir -p "$$CODEHUB_REGISTRY_E2E_DIR" && cargo run --locked -p e2e --example garam_keys -- "$$CODEHUB_REGISTRY_E2E_DIR" && \
	docker compose --env-file dev/test.env --profile registry up --detach --wait --wait-timeout 90 --no-recreate registry && \
	cargo nextest run -p e2e --locked --no-tests=fail --test registry

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

# The check services start while lint, format and build run: pulling their images and waiting for
# them to be healthy takes as long as those three together and needs no compile, and the tests after
# them need the services up. The three run one after another, in that order, in their own recipe.
ci:
	$(MAKE) -j2 services ci-compile
	$(MAKE) test e2e loadtest-smoke

.PHONY: ci-compile
ci-compile:
	$(MAKE) lint
	$(MAKE) format
	$(MAKE) build

# A fast local check, not the gate: `ci` stays the whole set. It lints the workspace, which only
# re-lints the packages changed since the last run and their dependents, and runs the tests of the
# packages that differ from origin/main (committed, staged, unstaged and untracked files) and of
# every package that depends on them. A change to a workspace-wide file runs every test. The e2e
# package and the doc tests are left to `ci`.
CHANGED_FILES := $(shell { git diff --name-only origin/main; git ls-files --others --exclude-standard; } | sort -u)
CHANGED_PACKAGES := $(shell printf '%s\n' $(CHANGED_FILES) | sed -n 's|^crates/\([^/]*\)/.*|\1|p' | sort -u)
CHANGED_WIDE := $(filter Cargo.toml Cargo.lock clippy.toml rust-toolchain.toml Makefile .config/% dev/test.env,$(CHANGED_FILES))
EMPTY :=
SPACE := $(EMPTY) $(EMPTY)

.PHONY: check-changed
check-changed: lint services
ifneq ($(CHANGED_WIDE),)
	@echo "check-changed: workspace-wide change ($(CHANGED_WIDE)); running every test"
	cargo nextest run --workspace --exclude e2e --locked --no-tests=fail $(TEST_ARGS)
else ifneq ($(CHANGED_PACKAGES),)
	@echo "check-changed: packages changed against origin/main: $(CHANGED_PACKAGES); testing them and their dependents"
	cargo nextest run --workspace --exclude e2e --locked --no-tests=fail -E '$(subst $(SPACE), | ,$(foreach p,$(CHANGED_PACKAGES),rdeps($(p))))' $(TEST_ARGS)
else
	@echo "check-changed: no package changed against origin/main; no test selected"
endif

# Mutation testing of the domain ring and the use cases of landing, write-back, merge and
# compare-and-swap (.cargo/mutants.toml names the files). A mutant the tests of `app` do not
# kill, or that times out, fails the run; the report is in target/mutants. It is not part of
# `ci`: the nightly workflow runs it, a shard at a time (MUTANTS_SHARD=0/4 runs the first of four).
MUTANTS_JOBS ?= 2
MUTANTS_SHARD ?=

mutants:
	mkdir -p target
	cargo mutants --package app --test-tool nextest --cargo-arg=--locked --jobs $(MUTANTS_JOBS) $(if $(MUTANTS_SHARD),--shard $(MUTANTS_SHARD)) --output target/mutants

# Running containers are kept; pick up a changed image or configuration with
# `make services-down` and then `make services`.
services:
	docker compose --env-file dev/test.env up --detach --wait --wait-timeout 180 --no-recreate

services-down:
	docker compose --env-file dev/test.env down --volumes
