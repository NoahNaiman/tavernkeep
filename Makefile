.PHONY: check fmt clippy test demo load build

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

build:
	cargo build --release

demo: build
	./scripts/demo.sh

load: build
	./scripts/load.sh
