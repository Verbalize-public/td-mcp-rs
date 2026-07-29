#!/usr/bin/env bash
# Local quality gate for td-mcp-rs (Unix).
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== cargo fmt --check =="
cargo fmt --all -- --check

echo "== cargo clippy =="
cargo clippy --workspace --all-targets -- -D warnings

echo "== cargo test =="
cargo test --workspace

echo "OK: check green"
