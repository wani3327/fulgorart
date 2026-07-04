FROM rust:1-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p fulgorart-tagger --features gcp

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        tar \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN curl -fL https://github.com/microsoft/onnxruntime/releases/download/v1.27.0/onnxruntime-linux-x64-1.27.0.tgz \
    | tar -xzf - --strip-components=1 "lib"
RUN curl -fLo wd-swinv2.onnx https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3/resolve/main/model.onnx?download=true
RUN curl -fLo selected_tags https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3/resolve/main/selected_tags.csv?download=true

COPY --from=builder /app/target/release/fulgorart-tagger /usr/local/bin/fulgorart-tagger

ENV ORT_DYLIB_PATH=/app/lib/libonnxruntime.so \
    WD14_MODEL_PATH=/app/wd-swinv2.onnx \
    WD14_LABELS_PATH=/app/selected_tags.csv

ENTRYPOINT ["fulgorart-tagger"]