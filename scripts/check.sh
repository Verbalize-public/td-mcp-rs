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

echo "== bridge python tests =="
python3 -m unittest discover -s bridge/tests -p "test_*.py" -v

echo "OK: check green"
