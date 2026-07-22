#!/usr/bin/env bash
set -euo pipefail

# Stops and removes the demo localstack. Scoped to the comhad-demo compose project, so it
# can't touch any other container.

cd "$(dirname "$0")/../.."

docker compose -f assets/demo/docker-compose.yml down -v
rm -rf /tmp/comhad-demo

echo "demo container and /tmp/comhad-demo removed."
echo "the bookmark at ~/.comhad/bookmarks/demo-localstack.json is left alone — delete it if you want it gone."
