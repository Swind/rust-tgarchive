#!/usr/bin/env sh
# Interactive Telegram login using credentials from .env (run from any directory).
set -eu

cd "$(dirname "$0")/.."

if [ ! -f .env ]; then
    echo "error: .env not found in $(pwd)" >&2
    exit 1
fi

set -a
. ./.env
set +a

: "${TELEGRAM_API_ID:?TELEGRAM_API_ID is missing in .env}"
: "${TELEGRAM_API_HASH:?TELEGRAM_API_HASH is missing in .env}"

cargo build --quiet
bin=target/debug/tgarchive

"$bin" db init

if [ -n "${TELEGRAM_PHONE:-}" ]; then
    "$bin" auth login --phone "$TELEGRAM_PHONE"
else
    "$bin" auth login
fi

echo "Session file: ${TELEGRAM_SESSION_FILE:-telegram.session}"
