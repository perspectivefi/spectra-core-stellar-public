.PHONY: all build test lint fmt fmt-check clean

all: build

build:
	./scripts/build-wasm.sh

test: build
	cargo test --workspace

lint:
	cargo clippy --workspace --all-targets -- -D warnings -A dead_code -A unused_imports

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clean:
	cargo clean
