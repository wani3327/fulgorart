FROM rust:1-bookworm AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p fulgorart-cli

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        jq \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/fulgorart-cli /usr/local/bin/fulgorart-cli
COPY crates/cli/utils/import_using_gallery_dl.sh /usr/local/bin/import_using_gallery_dl

RUN ln -s /usr/local/bin/fulgorart-cli /usr/local/bin/cli
RUN ln -s /usr/local/bin/import_using_gallery_dl /usr/local/bin/iugd
