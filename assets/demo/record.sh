#!/usr/bin/env bash
set -euo pipefail

# Records one of the README demo GIFs against a throwaway localstack container.
#
#   ./assets/demo/record.sh                            # the hero GIF
#   ./assets/demo/record.sh assets/demo/sync.tape      # any other tape
#
# Nothing here touches your real ~/.comhad, your AWS credentials, or any localstack you
# already have running (this one lives on port 4577 under its own container name).

cd "$(dirname "$0")/../.."

TAPE="${1:-assets/demo/demo.tape}"
COMPOSE="assets/demo/docker-compose.yml"

command -v vhs >/dev/null || { echo "vhs is not installed: brew install vhs" >&2; exit 1; }

# Refuse to start if anything already holds the port — a half-started container silently
# pointing the seed script at someone else's service is exactly the failure to avoid.
if lsof -nP -iTCP:24566 -sTCP:LISTEN >/dev/null 2>&1; then
    echo "port 24566 is already in use — refusing to start the demo container." >&2
    echo "Free it, or change the published port in $COMPOSE and bookmarks/localstack.json." >&2
    exit 1
fi

docker compose -f "$COMPOSE" up -d --wait
trap 'docker compose -f "$COMPOSE" down -v >/dev/null 2>&1 || true' EXIT

./assets/demo/seed.sh
cargo build --release

# A fixed, tidy path rather than mktemp -d: this shows up on camera in the download
# confirmation dialog, and /var/folders/qq/3f9x... does not look good in a README.
DEMO_HOME="/tmp/comhad-demo"
rm -rf "$DEMO_HOME"
mkdir -p "$DEMO_HOME/.comhad/bookmarks"
cp assets/demo/bookmarks/localstack.json "$DEMO_HOME/.comhad/bookmarks/"

# Fills $DEMO_HOME/Downloads (where the local pane opens) with a near-mirror of the bucket,
# so the sync dialog has every diff state in it.
./assets/demo/seed-local.sh "$DEMO_HOME"

HOME="$DEMO_HOME" PATH="$PWD/target/release:$PATH" vhs "$TAPE"
rm -rf "$DEMO_HOME"

# VHS writes a full-palette GIF; a TUI needs nowhere near 256 colours, and dropping to 64
# roughly halves the file for a README with no visible loss. Optional — skipped if gifsicle
# isn't installed (brew install gifsicle).
GIF="assets/$(basename "${TAPE%.tape}").gif"
[ -f "$GIF" ] || GIF=assets/demo.gif
if command -v gifsicle >/dev/null && [ -f "$GIF" ]; then
    before=$(du -h "$GIF" | cut -f1)
    gifsicle -O3 --lossy=100 --colors 64 "$GIF" -o "$GIF.opt" && mv "$GIF.opt" "$GIF"
    echo "optimised $GIF: $before -> $(du -h "$GIF" | cut -f1)"
fi

ls -lh assets/*.gif
