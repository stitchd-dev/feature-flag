#!/usr/bin/env bash
# ─── Local dev launcher ───────────────────────────────────────────────────────
# Starts all Stitchd microservices natively (cargo run).
# Databases (Postgres, ClickHouse, ScyllaDB) must be running in Docker:
#   docker compose up -d postgres clickhouse scylladb
#
# Usage:
#   ./dev.sh          # start all services
#   ./dev.sh stop     # kill all background services
#   ./dev.sh logs     # tail combined log output
#   ./dev.sh <svc>    # start only one service  (e.g. ./dev.sh gateway)

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOGS_DIR="$ROOT/.dev-logs"
PIDS_FILE="$ROOT/.dev-pids"

# Load env vars
# shellcheck source=.env.local
source "$ROOT/.env.local"

mkdir -p "$LOGS_DIR"

# ── Helpers ───────────────────────────────────────────────────────────────────

start_service() {
  local name="$1"
  local bin="$2"
  local log="$LOGS_DIR/${name}.log"

  if pgrep -f "target.*${bin}" > /dev/null 2>&1; then
    echo "  ⚠️  ${name} already running — skipping"
    return
  fi

  echo "  ▶  ${name}"
  cargo run --release -p "${bin}" > "$log" 2>&1 &
  local pid=$!
  echo "${pid} ${name}" >> "$PIDS_FILE"
}

stop_all() {
  if [[ ! -f "$PIDS_FILE" ]]; then
    echo "No PID file found — nothing to stop."
    return
  fi
  echo "Stopping services…"
  while IFS=' ' read -r pid name; do
    if kill "$pid" 2>/dev/null; then
      echo "  ■  ${name} (pid ${pid})"
    fi
  done < "$PIDS_FILE"
  rm -f "$PIDS_FILE"
  echo "Done."
}

# ── Command dispatch ──────────────────────────────────────────────────────────

CMD="${1:-all}"

case "$CMD" in
  stop)
    stop_all
    exit 0
    ;;
  logs)
    tail -f "$LOGS_DIR"/*.log
    exit 0
    ;;
  logs:*)
    svc="${CMD#logs:}"
    tail -f "$LOGS_DIR/${svc}.log"
    exit 0
    ;;
esac

# ── Build first (all at once, faster than per-service) ────────────────────────
echo "Building services…"
cargo build --release \
  -p stitchd-auth-service \
  -p stitchd-flag-service \
  -p stitchd-segmentation-service \
  -p stitchd-analytics-service \
  -p stitchd-experimentation-service \
  -p stitchd-stats-service \
  -p stitchd-gateway
echo "Build complete."
echo ""

# Clear stale PID file
rm -f "$PIDS_FILE"
touch "$PIDS_FILE"

echo "Starting services (logs → .dev-logs/)…"

case "$CMD" in
  all)
    start_service "auth-service"           "stitchd-auth-service"
    start_service "flag-service"           "stitchd-flag-service"
    start_service "segmentation-service"   "stitchd-segmentation-service"
    start_service "analytics-service"      "stitchd-analytics-service"
    start_service "experimentation-service" "stitchd-experimentation-service"
    start_service "stats-service"          "stitchd-stats-service"
    start_service "gateway"               "stitchd-gateway"
    ;;
  auth)        start_service "auth-service"           "stitchd-auth-service" ;;
  flag)        start_service "flag-service"           "stitchd-flag-service" ;;
  segmentation) start_service "segmentation-service"  "stitchd-segmentation-service" ;;
  analytics)   start_service "analytics-service"      "stitchd-analytics-service" ;;
  experimentation) start_service "experimentation-service" "stitchd-experimentation-service" ;;
  stats)       start_service "stats-service"          "stitchd-stats-service" ;;
  gateway)     start_service "gateway"               "stitchd-gateway" ;;
  *)
    echo "Unknown command: $CMD"
    echo "Usage: $0 [all|stop|logs|logs:<svc>|auth|flag|segmentation|analytics|experimentation|stats|gateway]"
    exit 1
    ;;
esac

echo ""
echo "Services started. Endpoints:"
echo "  Gateway API   → http://localhost:${GATEWAY_PORT}"
echo "  Auth gRPC     → localhost:${AUTH_SERVICE_PORT}"
echo "  Flag gRPC     → localhost:${FLAG_SERVICE_PORT}"
echo "  Segment gRPC  → localhost:${SEGMENTATION_SERVICE_PORT}"
echo "  Analytics gRPC→ localhost:${ANALYTICS_GRPC_PORT}"
echo "  Exp gRPC      → localhost:${EXPERIMENTATION_SERVICE_PORT}"
echo "  Stats gRPC    → localhost:${STATS_SERVICE_PORT}"
echo "  Stats HTTP    → http://localhost:${STATS_HTTP_PORT}"
echo ""
echo "  ./dev.sh stop       — stop all services"
echo "  ./dev.sh logs       — tail all logs"
echo "  ./dev.sh logs:gateway — tail gateway only"
