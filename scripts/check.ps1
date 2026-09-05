#!/usr/bin/env pwsh
# Local quality gate for td-mcp-rs (Windows).
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

Write-Host "== cargo fmt --check =="
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== cargo clippy =="
cargo clippy --locked --workspace --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== cargo test =="
cargo test --locked --workspace
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== bridge python tests (pytest) =="
python -m pytest bridge/tests -v
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

python scripts/check_docs.py
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

python -m unittest discover -s scripts/tests
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "OK: check green"
