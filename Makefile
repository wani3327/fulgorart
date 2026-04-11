.PHONY: build build-tagger check test run-web run-cli run-tagger run-ingestor fmt lint clean

build:
	cargo build

build-tagger:
	cargo build -p fulgorart-tagger

check:
	cargo check

test:
	cargo test

run-web:
	cargo run --bin fulgorart-web

run-cli:
	cargo run --bin fulgorart-cli -- --help

run-tagger:
	ORT_DYLIB_PATH=/home/ubuntu/fulgorart/onnxruntime-linux-x64-1.24.4/lib/libonnxruntime.so cargo run --bin fulgorart-tagger

run-ingestor:
	cargo run --bin fulgorart-ingestor

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets --all-features -- -D warnings

clean:
	cargo clean
