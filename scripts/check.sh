#!/usr/bin/env sh
set -eu

cargo fmt --all -- --check
# If the openapi.yml test fails, regenerate: cargo run -q -- openapi --format yaml > openapi.yml
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings

# Web UI (skipped when Node is unavailable; web/dist is committed so Rust builds never need it)
if command -v npm >/dev/null 2>&1; then
  npm --prefix web ci
  npm --prefix web run typecheck
  npm --prefix web run lint
  npm --prefix web test
  npm --prefix web run build
  git diff --exit-code -- web/dist || { echo "web/dist is stale: commit the rebuilt files" >&2; exit 1; }
  [ -z "$(git ls-files --others --exclude-standard web/dist)" ] || { echo "web/dist has untracked files" >&2; exit 1; }
else
  echo "npm not found: skipping web checks"
fi

# Browser E2E (optional: E2E=1 and docker; uses the Playwright image, see README)
if [ "${E2E:-0}" = "1" ]; then
  if command -v docker >/dev/null 2>&1; then
    scripts/e2e.sh
  else
    echo "docker not found: skipping E2E" >&2
  fi
fi
