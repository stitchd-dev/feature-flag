#!/usr/bin/env bash
# reset_dev_db.sh
#
# Reset the local development databases to a clean, fully-migrated state that
# matches CI's fresh-from-scratch behaviour.
#
# WHY THIS EXISTS (feature-flag-7rp):
#   The dev Postgres DB drifts from the branch migrations over time: a migration
#   file edited after it was applied leaves a "different checksum" on the
#   recorded baseline, and `sqlx migrate run` then refuses to apply the pending
#   migrations behind it (checksum-mismatch halt). CI never hits this because it
#   provisions a brand-new Postgres container every run. The fix is to DROP the
#   database entirely and recreate it from the V1 baseline — there is no
#   in-place "re-checksum" that is safe, and drift means local `cargo test`
#   (against the dev DB) and `cargo sqlx prepare` silently diverge from CI.
#
# WHAT IT DOES:
#   Postgres (default):    sqlx database drop -> create -> migrate run
#   ClickHouse (--all):    DROP/CREATE the `stitchd` CH database, then re-run the
#                          three CH migration sets via the event-writer migrator.
#   ScyllaDB  (--all):     truncate + re-apply CQL migrations via xtask.
#
# IDEMPOTENT + NON-INTERACTIVE: safe to run repeatedly; never prompts.
#
# Usage (from workspace root):
#   scripts/reset_dev_db.sh            # Postgres only (the common case)
#   scripts/reset_dev_db.sh --all      # Postgres + ClickHouse + ScyllaDB
#   scripts/reset_dev_db.sh --help
#
# Connection: reads STITCHD_DATABASE_URL (project convention) if set, else
# falls back to the docker-compose default. sqlx-cli needs a plain DATABASE_URL,
# which this script derives automatically.
#
# Exit codes: 0 = success, non-zero = a step failed (the failing step is named).

set -euo pipefail

# ── Resolve the workspace root (this script lives in <root>/scripts/) ──────────
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# ── Defaults (mirror docker-compose.yml) ──────────────────────────────────────
DEFAULT_PG_URL="postgres://stitchd:stitchd@localhost:5432/stitchd"
DEFAULT_CH_URL="http://localhost:8123"
CH_DB="stitchd"

RESET_ALL=0
for arg in "$@"; do
  case "$arg" in
    --all) RESET_ALL=1 ;;
    -h|--help)
      sed -n '2,40p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "✗ unknown argument: $arg (try --help)" >&2
      exit 2
      ;;
  esac
done

# sqlx-cli requires a plain DATABASE_URL (NOT the STITCHD_-prefixed var). Derive
# it from the project's STITCHD_DATABASE_URL when present so a single source of
# truth drives both. (Documented in conductor/workflow.md "Setup".)
PG_URL="${STITCHD_DATABASE_URL:-${DATABASE_URL:-$DEFAULT_PG_URL}}"
export DATABASE_URL="$PG_URL"

echo "▶ Resetting dev databases against:"
echo "    Postgres : $PG_URL"
[ "$RESET_ALL" -eq 1 ] && echo "    ClickHouse: ${STITCHD_CLICKHOUSE_URL:-$DEFAULT_CH_URL} (db: $CH_DB)"
echo

# ── Postgres: drop -> create -> migrate ───────────────────────────────────────
# `sqlx database drop -y` is non-interactive. Dropping the whole database is the
# ONLY reliable way to clear a checksum-mismatched baseline — there is no
# in-place re-checksum. Recreating then runs every migration from the V1
# baseline, exactly as CI does on a fresh container.
echo "── Postgres ──────────────────────────────────────────────"
echo "  • drop"
sqlx database drop -y --database-url "$PG_URL"
echo "  • create"
sqlx database create --database-url "$PG_URL"
echo "  • migrate (from V1 baseline)"
sqlx migrate run --source crates/stitchd-db/migrations --database-url "$PG_URL"
echo "  ✓ Postgres reset complete"
echo

if [ "$RESET_ALL" -eq 1 ]; then
  # ── ClickHouse: drop + recreate the database, then re-run CH migrations ──────
  # ClickHouse's HTTP interface rejects anonymous DDL (403) — the dev server
  # provisions a `stitchd` user. Derive credentials (default to the
  # docker-compose values) and pass them to both curl and the xtask migrator.
  CH_URL="${STITCHD_CLICKHOUSE_URL:-$DEFAULT_CH_URL}"
  CH_USER="${STITCHD_CLICKHOUSE_USER:-stitchd}"
  CH_PASSWORD="${STITCHD_CLICKHOUSE_PASSWORD:-stitchd}"
  CH_DB="${STITCHD_CLICKHOUSE_DB:-$CH_DB}"
  echo "── ClickHouse ────────────────────────────────────────────"
  ch_query() {
    curl -fsS "$CH_URL" --user "${CH_USER}:${CH_PASSWORD}" --data-binary "$1"
  }

  echo "  • drop + create database '$CH_DB'"
  # `SYNC` forces synchronous removal of Replicated*MergeTree metadata from
  # ClickHouse Keeper. Without it, a plain DROP leaves the replica registered at
  # /clickhouse/tables/<db>/<table>/replicas/<host>, and recreating the table on
  # migrate fails with REPLICA_ALREADY_EXISTS (Code 253).
  ch_query "DROP DATABASE IF EXISTS ${CH_DB} SYNC" >/dev/null

  # Belt-and-braces: even after a SYNC drop, a replica orphaned by an earlier
  # NON-sync drop (or an interrupted run) can linger in Keeper under
  # /clickhouse/tables/<db>/<table>/replicas/<replica>, which still triggers
  # REPLICA_ALREADY_EXISTS on recreate. Enumerate any leftover table paths and
  # drop their replica registration before recreating. `{replica}` resolves to
  # the server's replica macro (here: the host).
  CH_REPLICA="$(ch_query "SELECT getMacro('replica')")"
  ORPHANS="$(ch_query "SELECT name FROM system.zookeeper WHERE path='/clickhouse/tables/${CH_DB}'" || true)"
  if [ -n "$ORPHANS" ]; then
    echo "  • purging $(echo "$ORPHANS" | wc -l | tr -d ' ') orphaned Keeper replica(s)"
    while IFS= read -r tbl; do
      [ -z "$tbl" ] && continue
      ch_query "SYSTEM DROP REPLICA '${CH_REPLICA}' FROM ZKPATH '/clickhouse/tables/${CH_DB}/${tbl}'" >/dev/null || true
    done <<< "$ORPHANS"
  fi

  ch_query "CREATE DATABASE IF NOT EXISTS ${CH_DB}" >/dev/null
  echo "  • migrate (event-writer applies the canonical CH migration set)"
  # The event-writer's migrator owns the canonical CH migration order.
  # Invoking it via the ch-migrate xtask keeps one source of truth for CH DDL.
  STITCHD_CLICKHOUSE_URL="$CH_URL" \
  STITCHD_CLICKHOUSE_DB="$CH_DB" \
  STITCHD_CLICKHOUSE_USER="$CH_USER" \
  STITCHD_CLICKHOUSE_PASSWORD="$CH_PASSWORD" \
    cargo run --quiet --manifest-path crates/xtask/Cargo.toml -- ch-migrate
  echo "  ✓ ClickHouse reset complete"
  echo

  # ── ScyllaDB: re-apply CQL migrations ───────────────────────────────────────
  echo "── ScyllaDB ──────────────────────────────────────────────"
  cargo run --quiet --manifest-path crates/xtask/Cargo.toml -- scylla-migrate
  echo "  ✓ ScyllaDB migrations applied"
  echo
fi

echo "✓ Dev database reset complete — matches CI fresh-from-scratch state."
echo "  Verify with: DATABASE_URL='$PG_URL' sqlx migrate info --source crates/stitchd-db/migrations"
