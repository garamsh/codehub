.PHONY: lint format test build ci

lint:
	CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --locked

format:
	cargo fmt --all --check

test:
	sh scripts/test-count.sh

build:
	cargo build --workspace --all-targets --locked

ci: lint format test build
