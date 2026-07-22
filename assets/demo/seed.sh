#!/usr/bin/env bash
set -euo pipefail

# Seeds the demo bucket in the throwaway localstack container.
#
# Deliberately hermetic: no AWS profile, dummy static credentials, and an explicit local
# endpoint, so nothing here can reach real AWS or pick up your everyday credentials.
unset AWS_PROFILE AWS_DEFAULT_PROFILE
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export AWS_DEFAULT_REGION=us-east-1
export AWS_EC2_METADATA_DISABLED=true
# Modern AWS CLI adds a CRC32 trailer to every PutObject; localstack 2.1.0 predates that and
# rejects it with "x-amz-trailer is not supported".
export AWS_REQUEST_CHECKSUM_CALCULATION=when_required
export AWS_RESPONSE_CHECKSUM_VALIDATION=when_required

ENDPOINT="http://127.0.0.1:24566"
BUCKET="comhad-demo"
CONTAINER="comhad-demo-localstack"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# Never seed something that isn't ours. Two checks: the container this repo starts must
# actually be running, and whatever answers on the port must identify itself as a localstack
# with S3 loaded. Anything else is a hard stop rather than a "press on and hope".
if [ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" != "true" ]; then
    echo "$CONTAINER is not running — refusing to send anything to $ENDPOINT" >&2
    exit 1
fi

echo "waiting for localstack on $ENDPOINT ..."
ready=false
for _ in $(seq 1 60); do
    if curl -fsS --max-time 2 "$ENDPOINT/_localstack/health" 2>/dev/null | grep -q '"s3"'; then
        ready=true
        break
    fi
    sleep 1
done
if [ "$ready" != true ]; then
    echo "nothing identifying as localstack answered on $ENDPOINT — refusing to seed" >&2
    exit 1
fi

mkdir -p "$STAGE"/{config,data/2024-01-15,data/2024-01-16,data/exports,reports/q4,src,images,archive/2023,archive/logs}

cat >"$STAGE/config/warehouse.json" <<'EOF'
{
  "warehouse": "analytics-eu",
  "version": 4,
  "region": "eu-west-1",
  "enabled": true,
  "retired_at": null,
  "connection": {
    "host": "warehouse.internal",
    "port": 5432,
    "database": "analytics",
    "pool": { "min": 2, "max": 16, "idle_timeout_s": 300 }
  },
  "schemas": ["raw", "staging", "marts"],
  "owners": [
    { "team": "data-platform", "slack": "#data-platform", "primary": true },
    { "team": "analytics", "slack": "#analytics", "primary": false }
  ],
  "retention": {
    "raw_days": 30,
    "staging_days": 90,
    "marts_days": null
  }
}
EOF

cat >"$STAGE/README.md" <<'EOF'
# Orders pipeline

Nightly ingest of storefront events into the analytics warehouse.

| Stage     | Owner        | Schedule   |
| --------- | ------------ | ---------- |
| ingest    | data-platform| 02:00 UTC  |
| transform | analytics    | 03:15 UTC  |
| export    | analytics    | 04:00 UTC  |

## Layout

- `data/` — raw newline-delimited events, partitioned by ingest date.
- `src/` — the pipeline itself.
- `reports/` — generated CSVs, published to the BI tool.
- `archive/` — cold storage. Nothing here is read by the pipeline.

Backfills are safe to re-run: every stage is idempotent on `(event_id, ingested_at)`.
EOF

cat >"$STAGE/pipeline.yaml" <<'EOF'
version: 2

source:
  bucket: orders-raw
  prefix: events/
  format: ndjson
  compression: gzip

stages:
  - name: ingest
    entrypoint: src/ingest.py
    schedule: "0 2 * * *"
    retries: 3
    timeout: 45m
    resources:
      cpu: 2
      memory: 4Gi

  - name: transform
    entrypoint: src/transform.rs
    depends_on: [ingest]
    schedule: "15 3 * * *"
    partitions:
      by: ingest_date
      lookback_days: 7

  - name: export
    depends_on: [transform]
    targets:
      - reports/q4/summary.csv
      - data/exports/customers.csv

alerting:
  on_failure: "#data-platform-alerts"
  page_after_retries: 3
EOF

cat >"$STAGE/summary.csv" <<'EOF'
region,orders,revenue_eur,avg_basket_eur,returns_pct
north,18422,412883.55,22.41,3.1
south,15109,331902.10,21.97,2.8
east,9884,204551.72,20.69,4.2
west,21077,498120.94,23.63,2.4
central,12630,286440.18,22.68,3.6
EOF

cat >"$STAGE/reports/q4/summary.csv" <<'EOF'
quarter,region,orders,revenue_eur,yoy_pct
2024Q4,north,18422,412883.55,8.4
2024Q4,south,15109,331902.10,-1.2
2024Q4,east,9884,204551.72,12.9
2024Q4,west,21077,498120.94,6.1
2024Q4,central,12630,286440.18,3.7
EOF

cat >"$STAGE/data/exports/summary.csv" <<'EOF'
ingest_date,events,rejected,duplicate,bytes
2024-01-15,184223,412,1189,48219553
2024-01-16,179004,388,1042,47110228
EOF

cat >"$STAGE/data/exports/customers.csv" <<'EOF'
customer_id,signed_up,region,orders,lifetime_eur
c-100418,2021-03-14,north,64,1482.20
c-100419,2021-03-14,west,12,318.75
c-100420,2021-03-15,east,3,71.40
c-100421,2021-03-16,south,41,996.03
c-100422,2021-03-16,central,7,164.88
EOF

cat >"$STAGE/data/exports/orders.csv" <<'EOF'
order_id,customer_id,placed_at,items,total_eur,status
o-8841203,c-100418,2024-01-15T08:12:04Z,7,42.18,delivered
o-8841204,c-100421,2024-01-15T08:12:39Z,2,11.90,delivered
o-8841205,c-100419,2024-01-15T08:13:02Z,14,88.44,returned
o-8841206,c-100422,2024-01-15T08:13:51Z,1,6.25,delivered
EOF

cat >"$STAGE/data/2024-01-15/events.ndjson" <<'EOF'
{"event_id":"e-4f21a8","type":"order.placed","order_id":"o-8841203","ts":"2024-01-15T08:12:04Z","items":7}
{"event_id":"e-4f21a9","type":"order.placed","order_id":"o-8841204","ts":"2024-01-15T08:12:39Z","items":2}
{"event_id":"e-4f21aa","type":"order.returned","order_id":"o-8841205","ts":"2024-01-15T09:44:11Z","reason":"damaged"}
{"event_id":"e-4f21ab","type":"order.placed","order_id":"o-8841206","ts":"2024-01-15T08:13:51Z","items":1}
EOF

cat >"$STAGE/data/2024-01-16/events.ndjson" <<'EOF'
{"event_id":"e-4f3c07","type":"order.placed","order_id":"o-8849118","ts":"2024-01-16T07:02:18Z","items":3}
{"event_id":"e-4f3c08","type":"order.cancelled","order_id":"o-8849119","ts":"2024-01-16T07:31:55Z","reason":"out_of_stock"}
{"event_id":"e-4f3c09","type":"order.placed","order_id":"o-8849120","ts":"2024-01-16T07:48:02Z","items":11}
EOF

cat >"$STAGE/src/ingest.py" <<'EOF'
"""Pull raw storefront events out of S3 and land them as partitioned NDJSON."""

import gzip
import json
import logging
from dataclasses import dataclass
from datetime import date

LOGGER = logging.getLogger(__name__)

REJECTED = "rejected/"
BATCH_SIZE = 5_000


@dataclass(frozen=True)
class Event:
    event_id: str
    type: str
    order_id: str
    ts: str

    @classmethod
    def parse(cls, line: bytes) -> "Event | None":
        try:
            raw = json.loads(line)
        except json.JSONDecodeError:
            LOGGER.warning("skipping malformed line: %r", line[:80])
            return None
        return cls(raw["event_id"], raw["type"], raw["order_id"], raw["ts"])


def ingest(client, bucket: str, prefix: str, day: date) -> int:
    """Ingest one day's events, returning the number of rows written."""
    written = 0
    for key in client.list(bucket, f"{prefix}{day:%Y/%m/%d}/"):
        with gzip.open(client.get(bucket, key)) as fh:
            batch = [e for e in map(Event.parse, fh) if e is not None]
            written += client.put_ndjson(bucket, f"data/{day}/events.ndjson", batch)
    LOGGER.info("ingested %d events for %s", written, day)
    return written
EOF

cat >"$STAGE/src/transform.rs" <<'EOF'
//! Fold raw events into per-region daily aggregates.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub order_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub items: Option<u32>,
}

#[derive(Debug, Default, Serialize)]
pub struct Aggregate {
    pub orders: u64,
    pub items: u64,
    pub returned: u64,
}

/// Rolls a day's events up by region. Unknown regions are folded into `"unknown"` rather
/// than dropped, so the totals always reconcile against the raw count.
pub fn aggregate(events: &[Event], region_of: &HashMap<String, String>) -> HashMap<String, Aggregate> {
    let mut out: HashMap<String, Aggregate> = HashMap::new();

    for event in events {
        let region = region_of.get(&event.order_id).cloned().unwrap_or_else(|| "unknown".into());
        let entry = out.entry(region).or_default();

        match event.kind.as_str() {
            "order.placed" => {
                entry.orders += 1;
                entry.items += u64::from(event.items.unwrap_or(0));
            }
            "order.returned" => entry.returned += 1,
            _ => {}
        }
    }

    out
}

pub fn write_parquet(path: &str, rows: &HashMap<String, Aggregate>) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(rows)?)
        .with_context(|| format!("failed to write {path}"))
}
EOF

cat >"$STAGE/src/schema.sql" <<'EOF'
-- Warehouse tables for the orders pipeline.

CREATE TABLE IF NOT EXISTS orders (
    order_id     TEXT PRIMARY KEY,
    customer_id  TEXT NOT NULL REFERENCES customers (customer_id),
    placed_at    TIMESTAMPTZ NOT NULL,
    items        INTEGER NOT NULL CHECK (items > 0),
    total_eur    NUMERIC(10, 2) NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX IF NOT EXISTS orders_placed_at_idx ON orders (placed_at DESC);
CREATE INDEX IF NOT EXISTS orders_customer_idx  ON orders (customer_id);

-- Daily rollup, refreshed by the transform stage.
CREATE MATERIALIZED VIEW IF NOT EXISTS daily_revenue AS
SELECT
    date_trunc('day', placed_at) AS day,
    count(*)                     AS orders,
    sum(total_eur)               AS revenue_eur,
    avg(total_eur)               AS avg_basket_eur
FROM orders
WHERE status <> 'cancelled'
GROUP BY 1
ORDER BY 1 DESC;
EOF

cat >"$STAGE/archive/logs/app.log" <<'EOF'
2024-01-16T02:00:01Z  INFO  ingest    starting run for 2024-01-16
2024-01-16T02:00:04Z  INFO  ingest    listed 41 source objects (1.2 GiB)
2024-01-16T02:11:52Z  WARN  ingest    skipping malformed line in events-0031.ndjson.gz
2024-01-16T02:38:20Z  INFO  ingest    ingested 179004 events, 388 rejected
2024-01-16T03:15:00Z  INFO  transform starting run for 2024-01-16
2024-01-16T03:29:47Z  ERROR transform region lookup timed out, retrying (1/3)
2024-01-16T03:30:19Z  INFO  transform aggregated 5 regions
2024-01-16T04:00:00Z  INFO  export    wrote reports/q4/summary.csv
EOF

# Inline image previews. ffmpeg comes along with vhs.
if command -v ffmpeg >/dev/null; then
    ffmpeg -loglevel error -f lavfi -i mandelbrot=size=800x600 -frames:v 1 "$STAGE/images/mandelbrot.png"
    ffmpeg -loglevel error -f lavfi -i testsrc2=size=640x480 -frames:v 1 "$STAGE/images/colorbars.png"
else
    echo "ffmpeg not found — skipping the image files (the image preview beat will have nothing to show)" >&2
fi

# Big enough that the transfer progress bar actually animates rather than blinking past.
dd if=/dev/urandom of="$STAGE/archive/2023/backup.tar.gz" bs=1m count=40 status=none

if ! aws --endpoint-url "$ENDPOINT" s3api head-bucket --bucket "$BUCKET" 2>/dev/null; then
    aws --endpoint-url "$ENDPOINT" s3 mb "s3://$BUCKET"
fi
aws --endpoint-url "$ENDPOINT" s3 sync --no-progress "$STAGE" "s3://$BUCKET"

echo
echo "seeded s3://$BUCKET:"
aws --endpoint-url "$ENDPOINT" s3 ls --recursive "s3://$BUCKET" | awk '{printf "  %8s  %s\n", $3, $4}'
