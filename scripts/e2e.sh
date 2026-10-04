#!/usr/bin/env sh
# Browser E2E tests (Playwright). Builds tgarchive, seeds a fixture DB (no Telegram), serves it
# with --query-only on a free loopback port and runs web/e2e inside the official Playwright image
# (pinned to the @playwright/test version) so fonts/browsers match the committed screenshots.
#
#   scripts/e2e.sh                          # run
#   scripts/e2e.sh --update-snapshots       # regenerate screenshot baselines (inside the container)
#   scripts/e2e.sh -g "search"              # any extra args go to `playwright test`
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
E2E="$ROOT/web/e2e"
VERSION=$(sed -n 's/.*"@playwright\/test": *"\([0-9][0-9.]*\)".*/\1/p' "$E2E/package.json")
IMAGE="mcr.microsoft.com/playwright:v${VERSION}-noble"

mkdir -p /tmp/opencode 2>/dev/null || true
WORK=$(mktemp -d "${TMPDIR:-/tmp}/tgarchive-e2e.XXXXXX")
SERVER_PID=""
CONTAINER="tgarchive-e2e-$$"
cleanup() {
  [ -z "$SERVER_PID" ] || kill "$SERVER_PID" 2>/dev/null || true
  command -v docker >/dev/null 2>&1 && docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

USE_DOCKER=1
if ! command -v docker >/dev/null 2>&1 || ! timeout 20 docker info >/dev/null 2>&1; then
  USE_DOCKER=0
  if ! command -v npx >/dev/null 2>&1 || [ ! -d "$E2E/node_modules/@playwright/test" ] \
    || ! ls "$HOME"/.cache/ms-playwright/chromium-* >/dev/null 2>&1; then
    echo "e2e: SKIPPED (docker unavailable and no local Playwright browsers; install docker or run 'npx playwright install chromium' in web/e2e after 'npm ci')" >&2
    exit 0
  fi
  echo "e2e: docker unavailable, using local Playwright (screenshot assertions are skipped)" >&2
fi

timeout 1500 cargo build --quiet --bin tgarchive --example seed_fixture
DB="$WORK/fixture.db"
timeout 120 cargo run --quiet --example seed_fixture -- "$DB"

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])' 2>/dev/null || echo $((20000 + $$ % 20000)))
TGARCHIVE_NO_DOTENV=1 DATABASE_URL="sqlite://$DB" timeout 900 "$ROOT/target/debug/tgarchive" serve --query-only --bind "127.0.0.1:$PORT" >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
i=0
until curl -fsS "http://127.0.0.1:$PORT/health/ready" >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -gt 60 ] || ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "e2e: server did not become ready" >&2
    cat "$WORK/server.log" >&2
    exit 1
  fi
  sleep 0.5
done
export BASE_URL="http://127.0.0.1:$PORT"

if [ "$USE_DOCKER" = 1 ]; then
  # node_modules is shared with the host dir: @playwright/test is pure JS, browsers live in the image.
  timeout 1200 docker run --rm --init --ipc=host --network host --name "$CONTAINER" \
    --user "$(id -u):$(id -g)" -e HOME=/tmp -e CI=1 -e BASE_URL \
    -v "$E2E:/e2e" -w /e2e "$IMAGE" \
    sh -c 'if [ ! -d node_modules/@playwright/test ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then npm ci --no-audit --no-fund; fi && exec npx playwright test "$@"' sh "$@"
else
  (cd "$E2E" && timeout 900 npx playwright test --ignore-snapshots "$@")
fi
