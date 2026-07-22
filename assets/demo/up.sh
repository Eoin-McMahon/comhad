#!/usr/bin/env bash
set -euo pipefail

# Starts the demo localstack and seeds it, then leaves it running so you can drive comhad
# by hand. record.sh is the same thing but tears the container down when it's finished.
#
#   ./assets/demo/up.sh     # start + seed
#   ./assets/demo/down.sh   # stop + remove

cd "$(dirname "$0")/../.."

COMPOSE="assets/demo/docker-compose.yml"

if lsof -nP -iTCP:24566 -sTCP:LISTEN >/dev/null 2>&1 &&
    [ "$(docker inspect -f '{{.State.Running}}' comhad-demo-localstack 2>/dev/null)" != "true" ]; then
    echo "port 24566 is held by something that isn't the demo container — refusing to start." >&2
    exit 1
fi

docker compose -f "$COMPOSE" up -d --wait

mkdir -p /tmp/comhad-demo/.comhad/bookmarks
cp assets/demo/bookmarks/*.json /tmp/comhad-demo/.comhad/bookmarks/

./assets/demo/seed.sh
./assets/demo/seed-local.sh /tmp/comhad-demo

cat <<'EOF'

ready:

  HOME=/tmp/comhad-demo ./target/release/comhad

Two bookmarks, each paired with its own local directory via `local_path`, so the local pane
lands in the right place on connect. `L` brings it into view, `c` switches between them.

  demo (localstack)          the whole bucket  <->  /tmp/comhad-demo/Downloads
      for browsing, previews, copy/paste, delete, downloads

  exports sync (localstack)  data/exports/     <->  /tmp/comhad-demo/work/exports
      four files, one per sync state, so `s` is readable at a glance:
        =  customers.csv     identical
        ~  orders.csv        edited locally
        +  q1-forecast.csv   local only, uploads
        -  summary.csv       remote only, skipped

Re-run ./assets/demo/seed.sh (remote) or ./assets/demo/seed-local.sh (local) to reset either
side after you've mangled it. Stop everything with ./assets/demo/down.sh
EOF
