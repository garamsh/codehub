.PHONY: lint format test build ci services services-down

lint:
	CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --locked

format:
	cargo fmt --all --check

# Starts the check services (compose.yaml), smoke-tests them, and runs the
# workspace tests with the CODEHUB_TEST_* variables of dev/check-env.sh.
# The services stay up afterwards; `make services-down` stops them.
test: services
	set -a && . dev/check-env.sh && set +a && sh scripts/test-count.sh

build:
	cargo build --workspace --all-targets --locked

ci: lint format test build

services:
	sh scripts/services-up.sh
	sh scripts/check-services.sh

services-down:
	docker compose down --volumes
