FROM rust:1.87-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN cargo build --release -p stitchd-gateway

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 wget \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/stitchd-gateway /usr/local/bin/stitchd-gateway

ENV GATEWAY_PORT=8080
ENV METRICS_PORT=9080
EXPOSE 8080 9080

ENTRYPOINT ["/usr/local/bin/stitchd-gateway"]
