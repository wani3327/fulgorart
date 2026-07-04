FROM rust:1-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        pkg-config \
        curl \
        tar \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fL https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-linux-x64-1.24.4.tgz \
    | tar -xzf - --strip-components=1 "onnxruntime-linux-x64-1.24.4/lib"
RUN curl -fLo wd-swinv2.onnx https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3/resolve/main/model.onnx?download=true
RUN curl -fLo selected_tags.csv https://huggingface.co/SmilingWolf/wd-swinv2-tagger-v3/resolve/main/selected_tags.csv?download=true

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p fulgorart-tagger --features gcp

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/fulgorart-tagger /usr/local/bin/fulgorart-tagger
COPY --from=builder /app/lib /app/lib
COPY --from=builder /app/wd-swinv2.onnx /app/wd-swinv2.onnx
COPY --from=builder /app/selected_tags.csv /app/selected_tags.csv

ENV ORT_DYLIB_PATH=/app/lib/libonnxruntime.so \
    WD14_MODEL_PATH=/app/wd-swinv2.onnx \
    WD14_LABELS_PATH=/app/selected_tags.csv

ENTRYPOINT ["fulgorart-tagger"]