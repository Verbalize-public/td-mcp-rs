# V2-0 R2 probe — spawn/close lifecycle timeline against a real install.
# Captures: graceful WM_CLOSE exit, spawn->window->bridge-handshake timings,
# deterministic pid identity match. Run against any baked project (has bootstrap).
# Observed 2026-08-25: window 6s; handshake 27s cold / 23s warm / 8s hot. Identity exact.
$ErrorActionPreference = "Stop"
if ($args.Count -lt 1) { "usage: r2-spawn-lifecycle.ps1 <toePath>"; exit 1 }
$toe = $args[0]
$daemon = "http://127.0.0.1:9860/mcp/tools/call"
$exe = "C:\Program Files\Derivative\TouchDesigner.2025.32460\bin\TouchDesigner.exe"

function Get-FleetRow([int]$Want) {
    try {
        $r = Invoke-RestMethod -Uri $daemon -Method Post -Body '{"name":"fleet","arguments":{}}' -ContentType "application/json" -TimeoutSec 3
        return $r.data.processes | Where-Object { $_.pid -eq $Want }
    } catch { return $null }
}

$t0 = Get-Date
$p = Start-Process -FilePath $exe -ArgumentList "`"$toe`"" -PassThru
"spawned pid=$($p.Id)"
$winAt = $null; $hs = $null
while (((Get-Date) - $t0).TotalSeconds -lt 90) {
    Start-Sleep -Milliseconds 1500
    $cur = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
    if ($null -eq $cur) { "DIED at $([int]((Get-Date)-$t0).TotalSeconds)s"; break }
    if (-not $winAt -and $cur.MainWindowTitle) { $winAt = [int]((Get-Date) - $t0).TotalSeconds; "window at ${winAt}s" }
    $row = Get-FleetRow $p.Id
    if ($row) { $hs = [int]((Get-Date) - $t0).TotalSeconds; "handshake at ${hs}s: $($row | ConvertTo-Json -Compress)"; break }
}
if ($hs) {
    $null = $p.CloseMainWindow()   # graceful ladder step 1 (clean projects exit <8s, no prompt)
    "graceful exit: $($p.WaitForExit(8000))"
}
