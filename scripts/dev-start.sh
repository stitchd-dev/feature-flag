#!/usr/bin/env bash
# Start all Stitchd microservices locally (infra must already be running via docker compose).
# Usage: ./scripts/dev-start.sh
# Logs go to /tmp/stitchd-logs/

set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOGS="/tmp/stitchd-logs"
mkdir -p "$LOGS"

DB="postgres://stitchd:stitchd@localhost:5432/stitchd"
CH="http://stitchd:stitchd@localhost:8123/stitchd"

stop_all() {
  echo "Stopping all stitchd services..."
  pkill -f "stitchd-" 2>/dev/null || true
}
trap stop_all EXIT INT TERM

echo "Starting stitchd services..."
cd "$ROOT"

# auth-service
DATABASE_URL="$DB" \
  AUTH_SERVICE_PORT=50051 \
  JWT_SECRET=dev-secret-change-in-prod \
  SUPERADMIN_EMAIL=admin@stitchd.io \
  SUPERADMIN_PASSWORD=changeme \
  RUST_LOG=info \
  cargo run --release -p stitchd-auth-service >"$LOGS/auth-service.log" 2>&1 &
echo "  auth-service     → :50051 (log: $LOGS/auth-service.log)"

sleep 2  # give auth-service a head-start (others depend on it)

# flag-service
DATABASE_URL="$DB" \
  FLAG_SERVICE_PORT=50052 \
  RUST_LOG=info \
  cargo run --release -p stitchd-flag-service >"$LOGS/flag-service.log" 2>&1 &
echo "  flag-service     → :50052"

# segmentation-service
DATABASE_URL="$DB" \
  SEGMENTATION_SERVICE_PORT=50053 \
  RUST_LOG=info \
  cargo run --release -p stitchd-segmentation-service >"$LOGS/segmentation-service.log" 2>&1 &
echo "  segmentation     → :50053"

# event-service
DATABASE_URL="$DB" \
  CLICKHOUSE_URL="$CH" \
  EVENT_SERVICE_PORT=50054 \
  RUST_LOG=info \
  cargo run --release -p stitchd-event-service >"$LOGS/event-service.log" 2>&1 &
echo "  event-service    → :50054"

# experimentation-service
DATABASE_URL="$DB" \
  FLAG_SERVICE_ADDR=http://localhost:50052 \
  EXPERIMENTATION_SERVICE_PORT=50055 \
  RUST_LOG=info \
  cargo run --release -p stitchd-experimentation-service >"$LOGS/experimentation-service.log" 2>&1 &
echo "  experimentation  → :50055"

# stats-service
DATABASE_URL="$DB" \
  CLICKHOUSE_URL="$CH" \
  STATS_SERVICE_PORT=50056 \
  STATS_HTTP_PORT=9200 \
  RUST_LOG=info \
  cargo run --release -p stitchd-stats-service >"$LOGS/stats-service.log" 2>&1 &
echo "  stats-service    → :50056"

sleep 5  # wait for domain services before starting gateway

# gateway
GATEWAY_PORT=8080 \
  METRICS_PORT=9080 \
  AUTH_SERVICE_ADDR=http://localhost:50051 \
  FLAG_SERVICE_ADDR=http://localhost:50052 \
  SEGMENTATION_SERVICE_ADDR=http://localhost:50053 \
  EVENT_SERVICE_ADDR=http://localhost:50054 \
  EXPERIMENTATION_SERVICE_ADDR=http://localhost:50055 \
  STATS_SERVICE_ADDR=http://localhost:50056 \
  RUST_LOG=info \
  cargo run --release -p stitchd-gateway >"$LOGS/gateway.log" 2>&1 &
echo "  gateway          → :8080 (log: $LOGS/gateway.log)"

echo ""
echo "All services started. Gateway at http://localhost:8080"
echo "Logs in $LOGS/"
echo "Press Ctrl+C to stop all."

wait
