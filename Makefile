.PHONY: build build-tagger check test run-web run-cli run-tagger run-ingestor run-bridge

# Support: make run-tagger -- ./examples/eru.jpg
# and:     make run-tagger ARGS="./examples/eru.jpg"
ifeq (run-tagger,$(firstword $(MAKECMDGOALS)))
TAGGER_ARGS := $(wordlist 2,$(words $(MAKECMDGOALS)),$(MAKECMDGOALS))
$(eval $(TAGGER_ARGS):;@:)
endif

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
	ORT_DYLIB_PATH=/home/ubuntu/fulgorart/onnxruntime-linux-x64-1.24.4/lib/libonnxruntime.so cargo run --bin fulgorart-tagger -- $(or $(ARGS),$(TAGGER_ARGS))

run-ingestor:
	cargo run --bin fulgorart-ingestor

run-bridge:
	cargo run --bin fulgorart-bridge
