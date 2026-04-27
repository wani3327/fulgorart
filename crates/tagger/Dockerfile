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
COPY models ./models
COPY crates/tagger/onnxruntime-linux-x64-1.24.4 ./onnxruntime-linux-x64-1.24.4

RUN cargo build --release --locked -p fulgorart-tagger

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/fulgorart-tagger /usr/local/bin/fulgorart-tagger
COPY --from=builder /app/models /app/models
COPY --from=builder /app/onnxruntime-linux-x64-1.24.4/lib/libonnxruntime.so.1.24.4 /opt/onnxruntime/lib/libonnxruntime.so.1.24.4

RUN mkdir -p /app/data

# RUN ln -s /opt/onnxruntime/lib/libonnxruntime.so.1.24.4 /opt/onnxruntime/lib/libonnxruntime.so.1 \
#    && ln -s /opt/onnxruntime/lib/libonnxruntime.so.1 /opt/onnxruntime/lib/libonnxruntime.so

ENV ORT_DYLIB_PATH=/opt/onnxruntime/lib/libonnxruntime.so.1.24.4 \
    WD14_MODEL_PATH=/app/models/wd-swinv2.onnx \
    WD14_LABELS_PATH=/app/models/selected_tags.csv

ENTRYPOINT ["fulgorart-tagger"]