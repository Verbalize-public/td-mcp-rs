#!/usr/bin/env pwsh
# Soft-stop then force-kill workspace tdmcp-daemon processes that lock
# target/release or target/dist binaries (start + leftover mcp shims).
#
# Cursor may respawn `tdmcp-daemon mcp` if the MCP server is still connected —
# pause/reload MCP before rebuild when the binary stays locked after this script.
$ErrorActionPreference = "Continue"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

$port = if ($env:TDMCP_PORT) { $env:TDMCP_PORT } else { "9860" }
$shutdownUrl = "http://127.0.0.1:$port/admin/shutdown"
Write-Host "== soft stop $shutdownUrl =="
try {
    Invoke-WebRequest -Uri $shutdownUrl -Method POST -UseBasicParsing -TimeoutSec 2 | Out-Null
    Write-Host "shutdown requested"
} catch {
    Write-Host "soft stop skipped (daemon not reachable)"
}

Start-Sleep -Milliseconds 750

$exeName = "tdmcp-daemon.exe"
$targets = @(
    (Join-Path $root "target\release\$exeName"),
    (Join-Path $root "target\dist\$exeName")
) | ForEach-Object { [System.IO.Path]::GetFullPath($_) }

Write-Host "== force-kill workspace $exeName =="
$killed = 0
Get-CimInstance Win32_Process -Filter "Name = '$exeName'" -ErrorAction SilentlyContinue |
    ForEach-Object {
        $path = $_.ExecutablePath
        if (-not $path) { return }
        $full = try { [System.IO.Path]::GetFullPath($path) } catch { $null }
        if (-not $full) { return }
        $matched = $false
        foreach ($t in $targets) {
            if ($t.Equals($full, [System.StringComparison]::OrdinalIgnoreCase)) {
                $matched = $true
                break
            }
        }
        if (-not $matched) { return }
        Write-Host "kill pid=$($_.ProcessId) path=$full cmd=$($_.CommandLine)"
        Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        $killed++
    }

if ($killed -eq 0) {
    Write-Host "no matching workspace daemons"
} else {
    Write-Host "killed $killed process(es)"
}
exit 0
