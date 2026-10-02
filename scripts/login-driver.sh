#!/usr/bin/env sh
# One-time interactive login of the live-test DRIVER session (a second session of the same
# account, used only by tests/live_telegram.rs to send/edit/delete test messages).
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

driver_session="${TELEGRAM_DRIVER_SESSION_FILE:-./telegram-driver.session}"
if [ "$driver_session" = "${TELEGRAM_SESSION_FILE:-telegram.session}" ]; then
    echo "error: TELEGRAM_DRIVER_SESSION_FILE must differ from TELEGRAM_SESSION_FILE" >&2
    exit 1
fi
export TELEGRAM_SESSION_FILE="$driver_session"

cargo build --quiet
bin=target/debug/telegram-archive

if [ -n "${TELEGRAM_PHONE:-}" ]; then
    "$bin" auth login --phone "$TELEGRAM_PHONE"
else
    "$bin" auth login
fi

echo "Driver session file: $TELEGRAM_SESSION_FILE"
