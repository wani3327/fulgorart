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
        tar \
    && rm -rf /var/lib/apt/lists/*

# Tagger dependencies
RUN curl -fL \
        https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-linux-x64-1.24.4.tgz \
    | tar -xzf - --strip-components=1 \
        "onnxruntime-linux-x64-1.24.4/lib"
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