.PHONY: build check test run-web run-cli run-tagger run-ingestor fmt lint clean

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

run-tagger:
	cargo run --bin fulgorart-tagger

run-ingestor:
	cargo run --bin fulgorart-ingestor

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets --all-features -- -D warnings

clean:
	cargo clean
