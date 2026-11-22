# syntax=docker/dockerfile:1

# ============================================================
# 1. Chef
# ============================================================
FROM rust:1-bookworm AS chef

RUN cargo install cargo-chef --locked

WORKDIR /app


# ============================================================
# 2. Planner
# ============================================================
FROM chef AS planner

# Only copy files needed to determine the dependency graph.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo chef prepare --recipe-path recipe.json


# ============================================================
# 3. Builder
# ============================================================
FROM chef AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        pkg-config \
        curl \
        unzip \
    && rm -rf /var/lib/apt/lists/*

# Tagger dependencies
# ONNX Runtime native library, taken from the official PyPI wheel: the
# github.com/microsoft/onnxruntime release downloads are gone (404). Pinned by
# version and sha256; keep ORT_VERSION in line with the `ort` crate's api-NN.
ARG ORT_VERSION=1.24.4
ARG ORT_WHEEL_URL=https://files.pythonhosted.org/packages/6d/ab/5b68110e0460d73fad814d5bd11c7b1ddcce5c37b10177eb264d6a36e331/onnxruntime-1.24.4-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl
ARG ORT_WHEEL_SHA256=0d640eb9f3782689b55cfa715094474cd5662f2f137be6a6f847a594b6e9705c
RUN curl -fLo /tmp/ort.whl "$ORT_WHEEL_URL" \
    && echo "$ORT_WHEEL_SHA256  /tmp/ort.whl" | sha256sum -c - \
    && mkdir -p lib \
    && unzip -j -o /tmp/ort.whl "onnxruntime/capi/libonnxruntime.so.$ORT_VERSION" -d lib \
    && ln -sf "libonnxruntime.so.$ORT_VERSION" lib/libonnxruntime.so.1 \
    && ln -sf "libonnxruntime.so.$ORT_VERSION" lib/libonnxruntime.so \
    && rm /tmp/ort.whl
RUN curl -fLo wd-swinv2.onnx \
    'https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3/resolve/main/model.onnx?download=true'
RUN curl -fLo selected_tags.csv \
    'https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3/resolve/main/selected_tags.csv?download=true'

# First build only dependencies.
COPY --from=planner /app/recipe.json recipe.json

RUN cargo chef cook --release --locked --recipe-path recipe.json -p fulgorart-cli

# Now copy the actual source.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build the application itself.
RUN cargo build --release --locked -p fulgorart-cli


# ============================================================
# 4. Runtime
# ============================================================
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        jq \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Tagger dependencies
COPY --from=builder /app/lib /app/lib
COPY --from=builder /app/wd-swinv2.onnx /app/wd-swinv2.onnx
COPY --from=builder /app/selected_tags.csv /app/selected_tags.csv

# Binary
COPY --from=builder \
    /app/target/release/fulgorart-cli \
    /usr/local/bin/fulgorart-cli

# gallery-dl helper
COPY crates/cli/utils/import_using_gallery_dl.sh \
    /usr/local/bin/import_using_gallery_dl

ENV ORT_DYLIB_PATH=/app/lib/libonnxruntime.so \
    WD14_MODEL_PATH=/app/wd-swinv2.onnx \
    WD14_LABELS_PATH=/app/selected_tags.csv

RUN ln -s /usr/local/bin/fulgorart-cli /usr/local/bin/cli \
    && ln -s /usr/local/bin/import_using_gallery_dl /usr/local/bin/iugd