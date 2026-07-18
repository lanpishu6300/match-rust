# Multi-stage image for match-contract (memory transport / local smoke).
FROM rust:bookworm AS builder
WORKDIR /app
COPY rust-toolchain.toml ./
COPY . .
RUN rustup show && cargo build -p match-contract --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/match-contract /usr/local/bin/match-contract
COPY crates/match-contract/config.example.yaml /app/config.yaml
ENV MATCH_CONTRACT_CONFIG=/app/config.yaml
ENV MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
EXPOSE 31015
ENTRYPOINT ["match-contract"]
