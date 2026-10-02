#!/usr/bin/env sh
set -eu

cargo fmt --all -- --check
# If the openapi.yml test fails, regenerate: cargo run -q -- openapi --format yaml > openapi.yml
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
