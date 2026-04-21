FROM rust:1.87-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .
RUN cargo build --release -p stitchd-auth-service

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

# grpc_health_probe for Docker healthcheck
RUN apt-get update && apt-get install -y --no-install-recommends wget \
    && wget -qO /usr/local/bin/grpc_health_probe \
       https://github.com/grpc-ecosystem/grpc-health-probe/releases/download/v0.4.25/grpc_health_probe-linux-amd64 \
    && chmod +x /usr/local/bin/grpc_health_probe \
    && apt-get remove -y wget && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/stitchd-auth-service /usr/local/bin/stitchd-auth-service

ENV AUTH_SERVICE_PORT=50051
EXPOSE 50051

ENTRYPOINT ["/usr/local/bin/stitchd-auth-service"]
