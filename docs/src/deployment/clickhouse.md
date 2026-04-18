# ClickHouse Setup

Stitchd uses ClickHouse 24 or later for events, experiment results, and metric aggregations.

## Requirements

- ClickHouse 24+

## Setup

```bash
# Create the stitchd_events database
clickhouse-client --query "CREATE DATABASE stitchd_events"
```
