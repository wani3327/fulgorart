FROM rust:1-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked --package fulgorart-web

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/fulgorart-web /usr/local/bin/fulgorart-web
COPY --from=builder /app/crates/web/assets /app/crates/web/assets
COPY --from=builder /app/crates/web/templates /app/crates/web/templates

ENV FULGORART_PORT=3000

EXPOSE 3000

ENTRYPOINT ["fulgorart-web"]
