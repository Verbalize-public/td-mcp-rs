# V2-0 R1 probe — does toeexpand recurse into dragged-tox subtrees?
# Prereqs: daemon healthy on :9860, exactly one owned TD host connected (see docs/DEV_ENV.md).
# Verdict recorded 2026-08-25: dragged tox materializes INLINE as plain grammar files
# (.n/.parm/.text per op) - no opaque blobs. install_bridge update-existing is viable.
$ErrorActionPreference = "Stop"
$daemon = "http://127.0.0.1:9860/mcp/tools/call"
$bin = "C:\Program Files\Derivative\TouchDesigner.2025.32460\bin"
$r0 = "$PSScriptRoot\..\..\fixtures\v2-probes\r0"
New-Item -ItemType Directory -Force -Path $r0 | Out-Null

function Invoke-Td([int]$Pid2, [string]$Script) {
    $body = @{ name = "execute_python"; arguments = @{ pid = $Pid2; script = $Script } } | ConvertTo-Json -Depth 5
    (Invoke-RestMethod -Uri $daemon -Method Post -Body $body -ContentType "application/json" -TimeoutSec 90)
}

$row = (Invoke-RestMethod -Uri $daemon -Method Post -Body '{"name":"fleet","arguments":{}}' -ContentType "application/json").data.processes[0]
$pid2 = $row.pid
"host pid=$pid2 toe=$($row.toePath)"

# NOTE: project.save(path) is SAVE-AS (rebinds the session). Restore the host afterwards
# by saving back to its original path or re-spawning it (see r2 script).
$out = Join-Path $r0 "r1_live.toe"
$s = @"
import os
out = r"$out"
project.save(out)
result = {"saved": out, "bytes": os.path.getsize(out)}
"@
Invoke-Td $pid2 $s | Out-Null

& "$bin\toeexpand.exe" $out | Out-String
"toeexpand exit=$LASTEXITCODE  # unreliable: judge by artifacts only"
$dir = "$out.dir"
"entries=$(((Get-ChildItem $dir -Recurse -File)).Count)  tocExists=$(Test-Path "$out.toc")"
Get-ChildItem $dir -Recurse -File | ForEach-Object { $_.FullName.Replace("$dir\", "") } | Select-String -Pattern "tdmcp_rs|e2e_kit"
# Expect: tdmcp_rs\bootstrap.text, callbacks.text, tdmcp_exec.text, face ops -> inline expansion.
