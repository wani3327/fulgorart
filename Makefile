.PHONY: build check test run-web run-cli fmt lint clean

build:
	cargo build --release

check:
	cargo check

test:
	cargo test

run-web:
	cargo run --bin fulgorart-web

run-cli:
	cargo run --bin fulgorart-cli -- --help

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets --all-features -- -D warnings

clean:
	cargo clean
