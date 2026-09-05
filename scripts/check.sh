#!/usr/bin/env bash
# Local quality gate for td-mcp-rs (Unix).
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== cargo fmt --check =="
cargo fmt --all -- --check

echo "== cargo clippy =="
cargo clippy --locked --workspace --all-targets -- -D warnings

echo "== cargo test =="
cargo test --locked --workspace

echo "== bridge python tests (pytest) =="
python3 -m pytest bridge/tests -v

python3 scripts/check_docs.py
python3 -m unittest discover -s scripts/tests

echo "OK: check green"
