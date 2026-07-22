#!/usr/bin/env bash
set -euo pipefail

# Seeds the two local directories the demo bookmarks are paired with, via their `local_path`.
#
#   ~/Downloads    ← "demo (localstack)", a near-mirror of the whole bucket. Realistic, and
#                    what you want for browsing, copy/paste and delete.
#
#   ~/work/exports ← "exports sync (localstack)", deliberately tiny: exactly four files, one
#                    per sync state, so pressing `s` shows what sync does at a glance:
#
#                       =  customers.csv     identical
#                       ~  orders.csv        edited locally, three rows longer
#                       +  q1-forecast.csv   local only, will be uploaded
#                       -  summary.csv       remote only, shown greyed and skipped
#
# Hermetic, same as seed.sh: no AWS profile, dummy credentials, explicit local endpoint.

cd "$(dirname "$0")/../.."

unset AWS_PROFILE AWS_DEFAULT_PROFILE
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export AWS_DEFAULT_REGION=us-east-1
export AWS_EC2_METADATA_DISABLED=true
export AWS_REQUEST_CHECKSUM_CALCULATION=when_required
export AWS_RESPONSE_CHECKSUM_VALIDATION=when_required

ENDPOINT="http://127.0.0.1:24566"
BUCKET="comhad-demo"
DEMO_HOME="${1:-/tmp/comhad-demo}"
MIRROR="$DEMO_HOME/Downloads"
EXPORTS="$DEMO_HOME/work/exports"

if [ "$(docker inspect -f '{{.State.Running}}' comhad-demo-localstack 2>/dev/null)" != "true" ]; then
    echo "comhad-demo-localstack is not running — start it with ./assets/demo/up.sh" >&2
    exit 1
fi

# ── the browsing mirror: most of the bucket, a couple of deliberate differences ────────
rm -rf "$MIRROR"
mkdir -p "$MIRROR"
aws --endpoint-url "$ENDPOINT" s3 sync --no-progress "s3://$BUCKET" "$MIRROR" >/dev/null

rm -rf "$MIRROR/archive"
rm -f "$MIRROR/images/colorbars.png"
find "$MIRROR" -type f -exec touch -t 202401010000 {} +

printf 'islands,3401,74920.66,22.03,2.9\n' >>"$MIRROR/summary.csv"

# ── the sync folder: one file per state, nothing else to read past ────────────────────
rm -rf "$EXPORTS"
mkdir -p "$EXPORTS"

# `=` unchanged — byte-identical, and backdated so it isn't judged "newer" than the object.
aws --endpoint-url "$ENDPOINT" s3 cp --no-progress \
    "s3://$BUCKET/data/exports/customers.csv" "$EXPORTS/customers.csv" >/dev/null
touch -t 202401010000 "$EXPORTS/customers.csv"

# `~` update — the same file, three rows longer than the object.
aws --endpoint-url "$ENDPOINT" s3 cp --no-progress \
    "s3://$BUCKET/data/exports/orders.csv" "$EXPORTS/orders.csv" >/dev/null
cat >>"$EXPORTS/orders.csv" <<'EOF'
o-8849118,c-100420,2024-01-16T07:02:18Z,3,19.74,delivered
o-8849119,c-100418,2024-01-16T07:31:55Z,5,31.02,cancelled
o-8849120,c-100421,2024-01-16T07:48:02Z,11,74.60,delivered
EOF

# `+` add — local only.
cat >"$EXPORTS/q1-forecast.csv" <<'EOF'
quarter,region,forecast_orders,forecast_revenue_eur,confidence
2025Q1,north,19100,428500.00,0.82
2025Q1,south,15400,338200.00,0.79
2025Q1,east,10650,219800.00,0.71
2025Q1,west,21900,517400.00,0.85
2025Q1,central,12950,293100.00,0.77
EOF

# `-` extra — summary.csv is deliberately NOT copied, so it stays remote-only.

cat <<EOF

seeded:
  $MIRROR
      near-mirror of the bucket (archive/ and images/colorbars.png left out, summary.csv edited)
  $EXPORTS
      =  customers.csv     identical
      ~  orders.csv        3 rows longer than the object
      +  q1-forecast.csv   local only
      -  summary.csv       remote only, not copied here
EOF
