#!/usr/bin/env pwsh
# Local quality gate for td-mcp-rs (Windows).
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

Write-Host "== cargo fmt --check =="
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== cargo clippy =="
cargo clippy --workspace --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== cargo test =="
cargo test --workspace
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== bridge python tests =="
python -m unittest discover -s bridge/tests -p "test_*.py" -v
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "OK: check green"
