.PHONY: build build-tagger check test run-web run-cli run-tagger run-ingestor run-bridge docker-build-tagger docker-run-tagger docker-run-tagger-persist docker-tag-tagger docker-push-tagger docker-save-tagger docker-load-tagger

TAGGER_IMAGE_NAME ?= fulgorart-tagger
TAGGER_IMAGE_TAG ?= latest
TAGGER_IMAGE ?= $(TAGGER_IMAGE_NAME):$(TAGGER_IMAGE_TAG)
TAGGER_SAVE_FILE ?= fulgorart-tagger.tar

# Docker Hub push configuration.
DOCKERHUB_USER ?=
DOCKERHUB_IMAGE ?= $(DOCKERHUB_USER)/$(TAGGER_IMAGE_NAME):$(TAGGER_IMAGE_TAG)

# Support positional args for these targets:
#   make run-tagger -- ./examples/eru.jpg
#   make docker-run-tagger -- https://example.com/a.jpg
# Also supports ARGS="...".
ARG_TARGETS := run-tagger docker-run-tagger docker-run-tagger-persist
ifneq (,$(filter $(firstword $(MAKECMDGOALS)),$(ARG_TARGETS)))
TARGET_ARGS := $(wordlist 2,$(words $(MAKECMDGOALS)),$(MAKECMDGOALS))

# Swallow extra command goals when passing positional args, including URLs.
# Example: make docker-run-tagger https://example.com/a.jpg
%:
	@:
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
	ORT_DYLIB_PATH=/home/ubuntu/fulgorart/crates/tagger/onnxruntime-linux-x64-1.24.4/lib/libonnxruntime.so cargo run --bin fulgorart-tagger -- $(or $(ARGS),$(TARGET_ARGS))

run-ingestor:
	cargo run --bin fulgorart-ingestor

run-bridge:
	GOOGLE_APPLICATION_CREDENTIALS=development-493004-1f72f74562c7.json cargo run --bin fulgorart-bridge -- --tagger-mode cloud_run

docker-build-tagger:
	docker build -f crates/tagger/Dockerfile -t $(TAGGER_IMAGE) .

docker-run-tagger:
	docker run --rm --env-file .env $(TAGGER_IMAGE) $(or $(ARGS),$(TARGET_ARGS))

docker-run-tagger-persist:
	mkdir -p data
	docker run --rm --env-file .env -v "$$PWD/data:/app/data" $(TAGGER_IMAGE) $(or $(ARGS),$(TARGET_ARGS))

docker-tag-tagger:
	@test -n "$(DOCKERHUB_USER)" || (echo "Set DOCKERHUB_USER=<dockerhub-username>" && exit 1)
	docker tag $(TAGGER_IMAGE) $(DOCKERHUB_IMAGE)

docker-push-tagger: docker-tag-tagger
	docker push $(DOCKERHUB_IMAGE)

docker-save-tagger:
	docker save -o $(TAGGER_SAVE_FILE) $(TAGGER_IMAGE)

docker-load-tagger:
	docker load -i $(TAGGER_SAVE_FILE)
