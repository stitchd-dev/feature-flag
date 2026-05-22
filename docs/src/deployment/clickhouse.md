# ClickHouse Setup

ClickHouse 24+ stores everything that is high-throughput, append-only, and
analytical: raw event ingestion, the per-flag evaluation log,
experiment first-exposure assignments, pre-aggregated metric rollups, and the
per-iteration experiment statistical results.

Three Stitchd services touch ClickHouse:

| Service                          | Role                                                                                |
|----------------------------------|-------------------------------------------------------------------------------------|
| `stitchd-analytics-service`      | Writes `events` / `events_v2`; owns the `experiment_results` table.                 |
| `stitchd-flag-service`           | Writes one row to `flag_evaluation_log` per flag evaluation.                        |
| `stitchd-stats-service`          | Reads aggregated MVs + writes computed `experiment_results` rows on schedule.       |

The
[`clickhouse-rs`](https://docs.rs/clickhouse/) HTTP client is used everywhere;
there is no native-TCP path. The HTTP interface (`8123`) is the one that
matters for service traffic.

## System Requirements

- ClickHouse Server 24.x (matches the `clickhouse/clickhouse-server:24-alpine`
  image in [`docker-compose.yml`](https://github.com/stitchd-dev/feature-flag/blob/main/docker-compose.yml)).
- A single database owned by a single Stitchd role. Replication mode is
  enabled in every `ENGINE` line — see the [Replication](#replication--keeper)
  section.

## Schema Highlights

The current production tables, materialised views, and dictionaries:

| Object                              | Engine / Kind                                | Owner service           | Purpose                                                                                       |
|-------------------------------------|----------------------------------------------|-------------------------|-----------------------------------------------------------------------------------------------|
| `events`                            | `MergeTree`, monthly partitions              | `analytics-service`     | Original raw event ingestion table.                                                           |
| `events_v2`                         | `MergeTree`, weekly `toMonday()` partitions  | `analytics-service`     | Successor with finer partition granularity + `properties_json`.                               |
| `flag_evaluation_log`               | `MergeTree`, weekly `toMonday()` + 90d TTL   | `flag-service`          | One row per evaluation; carries `targeting_on` + `matched_rule_id`.                           |
| `experiment_assignments`            | `ReplacingMergeTree(_version)`               | _MV-derived_            | First-exposure assignment per `(experiment_id, iteration_id, context_type, context_key)`.    |
| `experiment_assignments_mv`         | Materialized View → `experiment_assignments` | _derived_               | Routes eval-log rows to assignments via `dictGet` on `experiment_iterations_active`.          |
| `experiment_iterations_active`      | `DICTIONARY (COMPLEX_KEY_HASHED)`            | `experimentation-service` | PostgreSQL-sourced lookup table for active experiment iterations.                            |
| `experiment_results`                | `MergeTree`                                  | `analytics-service`     | Per-iteration stats output written by `stats-service` on its 60-minute tick.                  |
| `metric_definitions`                | `ReplicatedReplacingMergeTree(updated_at)`   | `analytics-service`     | Mirror of PG `event_definitions` for query-time joins.                                        |
| `events_count` / `events_count_mv`  | `ReplicatedSummingMergeTree` + MV            | _derived_               | `(env_id, metric_key, day)` daily event counts.                                               |
| `events_numeric` / `events_numeric_mv` | `AggregatingMergeTree` + MV               | _derived_               | Sum / count / uniq state per `(env_id, metric_key, day)` for numeric event values.            |
| `events_experiment_daily` + `_mv`   | `AggregatingMergeTree` + MV                  | _derived_               | Pre-aggregated experiment stats by `(env_id, experiment_id, variant_key, metric_key, day)`.   |

### `flag_evaluation_log` attribution columns

Two columns drive server-derived experiment attribution
(`experimentation_full_20260521`):

- **`targeting_on Bool`** — replaces the older `is_disabled Bool`. The MV
  filters `WHERE targeting_on` so disabled-flag evaluations never become
  experiment exposures.
- **`matched_rule_id Nullable(UUID)`** — `UUID` when a custom rule matched,
  `NULL` when the flag fell through to its default rule (or targeting was
  off).

`experiment_assignments_mv` joins
`(env_id, flag_id, matched_rule_id, context_type)` against the
`experiment_iterations_active` dictionary to attribute eligible evaluations.
`NULL ↔ NULL` matching is supported by `COMPLEX_KEY_HASHED` on ClickHouse
24.10+. See the [Attribution Model](../experimentation/attribution.md) for
details.

## 1. Provision the Database

Compose handles this automatically via `CLICKHOUSE_USER` / `CLICKHOUSE_DB` /
`CLICKHOUSE_PASSWORD` on the `clickhouse` service. For a self-managed
deployment:

```sql
CREATE DATABASE stitchd;
CREATE USER stitchd IDENTIFIED WITH sha256_password BY 'your-strong-password';
GRANT ALL ON stitchd.* TO stitchd;
```

## 2. Run Migrations

There are **two** migration sets — both embedded in their owning crate and
applied via `include_str!`-driven runners. There is no external migration
binary for ClickHouse today; the runners are invoked from integration tests
and must be applied to production manually (see the [Gotchas](#gotchas)
section for details).

| Set                                                                                                                                  | Owning runner                                  | Tables created / altered                                                                            |
|--------------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------|------------------------------------------------------------------------------------------------------|
| [`crates/stitchd-event-writer/migrations/`](https://github.com/stitchd-dev/feature-flag/tree/main/crates/stitchd-event-writer/migrations) | `stitchd_event_writer::migrations::run`        | Events, MVs, flag-eval-log, experiment-assignments, dictionary.                                     |
| [`crates/stitchd-analytics-service/clickhouse-migrations/`](https://github.com/stitchd-dev/feature-flag/tree/main/crates/stitchd-analytics-service/clickhouse-migrations) | _Service-local migration; see crate sources_   | `experiment_results` (written by `stats-service`).                                                  |

The historical
[`crates/stitchd-db/clickhouse-migrations/`](https://github.com/stitchd-dev/feature-flag/tree/main/crates/stitchd-db/clickhouse-migrations)
directory is **reference-only** — files there are not wired into either
runner. New ClickHouse schema changes belong in `stitchd-event-writer/migrations/`.

Each runner uses an idempotent `_schema_migrations` tracking table inside
the target ClickHouse database; re-running is safe.

### Apply via tests / programmatically

The migrations are designed to run from Rust:

```rust
use clickhouse::Client;
use stitchd_event_writer::migrations;

let client = Client::default()
    .with_url("http://localhost:8123")
    .with_database("stitchd")
    .with_user("stitchd")
    .with_password("stitchd");

migrations::run(&client).await?;
```

In CI and integration tests this is invoked at test-suite boot. For
production deploys, today's pattern is to wrap the same call in a short
one-shot binary or run a `cargo test` against an empty database before the
first service starts.

## 3. Connection Configuration

Each service constructs its ClickHouse client from environment variables.
Defaults match the compose stack. See [`./env-vars.md`](./env-vars.md) for
the full list; the relevant ones are:

| Variable                     | Used by                                          | Example                              |
|------------------------------|--------------------------------------------------|--------------------------------------|
| `STITCHD_CLICKHOUSE_URL`     | analytics, stats, experimentation, flag service  | `http://localhost:8123`              |
| `STITCHD_CLICKHOUSE_DB`      | analytics, stats                                 | `stitchd`                            |
| `STITCHD_CLICKHOUSE_USER`    | analytics, stats                                 | `stitchd`                            |
| `STITCHD_CLICKHOUSE_PASSWORD`| analytics, stats                                 | `stitchd`                            |

The `stats-service` accepts a single combined URL of the form
`http://user:pass@host:8123/db` via `STITCHD_CLICKHOUSE_URL` — this mirrors
what the compose file passes through.

## Docker (Development)

The compose stack already mounts `docker/clickhouse/macros.xml` and
`docker/clickhouse/keeper.xml` into `/etc/clickhouse-server/config.d/` so
the `Replicated*` engine families have a `{replica}` macro and an embedded
ClickHouse Keeper to talk to. Start the data stores in isolation:

```bash
docker compose up -d --wait postgres clickhouse scylladb
```

To open a `clickhouse-client` shell against the running container:

```bash
docker exec -it stitchd-clickhouse clickhouse-client \
    --user stitchd --password stitchd --database stitchd
```

## Replication & Keeper

Every `ENGINE = MergeTree` line is actually a `Replicated*` family engine —
required because some tables (`metric_definitions`, `events_count`,
`events_numeric`) explicitly use `ReplicatedReplacingMergeTree` /
`ReplicatedSummingMergeTree`. The embedded Keeper config in
`docker/clickhouse/keeper.xml` makes this work for single-node dev. For a
multi-node production cluster, point `keeper.xml` at your real Keeper
ensemble and bump the `replication_factor` on the cluster macros.

## AggregatingMergeTree Invariants

Three rules govern the experiment-attribution and metric-rollup tables:

- **Insert:** use `*State` combiners (`countState()`, `sumState(Float64)`,
  `uniqState()`).
- **Read:** use `*Merge` combiners (`countMerge`, `sumMerge`, `uniqMerge`)
  in `GROUP BY` queries.
- `sumState(Nullable(Float64))` does **not** match the column type
  `AggregateFunction(sum, Float64)`. Wrap nullable inputs with
  `ifNull(value, 0.0)` before aggregating.

The `experiment_assignments` table is a `ReplacingMergeTree(_version)` with
a signed `_version = -toUnixTimestamp64Milli(evaluated_at)` column —
merges keep the row with the highest `_version`, i.e. the **earliest**
evaluation. Readers must collapse with `FINAL` or `argMin` to be
deduplication-safe.

## Gotchas

- **Migrations are not auto-applied on service boot.** `main.rs` for the
  analytics service constructs a ClickHouse client but does not call
  `stitchd_event_writer::migrations::run`. Apply migrations out-of-band on
  fresh deploys; otherwise the first write into `events` / `flag_evaluation_log`
  fails with `UNKNOWN_TABLE`.
- **Postgres comes first.** The `experiment_iterations_active` dictionary
  sources rows from `public.v_experiment_iterations_active` in Postgres
  (migration `20260521000004_v_experiment_iterations_active.sql`). If the
  ClickHouse dictionary refreshes before the Postgres view exists the
  dictionary load logs an error and the attribution MV becomes a no-op.
- **`host.docker.internal` is hard-coded** in the dictionary source. The
  dictionary uses `host 'host.docker.internal'` so it can reach the host's
  Postgres from inside the ClickHouse container. Production deployments
  must override this — patch the migration or `ALTER DICTIONARY` after
  apply to point at your real Postgres endpoint.
- **`flag_evaluation_log` was renamed from `flag_evaluation_log_v2`.** The
  v2 suffix was only a transitional naming during a partition-granularity
  migration; the current canonical table is `flag_evaluation_log`. The
  reference-only `crates/stitchd-db/clickhouse-migrations/0004_flag_evaluation_log_v2.sql`
  shows the historical DDL.
