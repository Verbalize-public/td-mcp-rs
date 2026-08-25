# V2-0 R3 probe — author a COMP into an expand dir, repack with toecollapse, verify live.
# Laws this encodes (2026-08-25): toc MUST be LF-only/no BOM (CRLF => silent 0-byte output,
# exit 0); toecollapse success judged by packed file size > 0 only; TD tolerates any toc
# position and re-derives canonical order itself.
$ErrorActionPreference = "Stop"
$daemon = "http://127.0.0.1:9860/mcp/tools/call"
$bin = "C:\Program Files\Derivative\TouchDesigner.2025.32460\bin"
$r0 = "$PSScriptRoot\..\..\fixtures\v2-probes\r0"

robocopy "$r0\r1_live_expanded.dir" "$r0\r3_authored.toe.dir" /E /NFL /NDL /NJH /NJS | Out-Null
if ($LASTEXITCODE -ge 8) { "robocopy failed"; exit 1 }

$node = "COMP:base`nv 500 500 0.5`ntile -75 250 160 130`nflags =  parlanguage 0`ncolor 0.67 0.67 0.67 `nend`n"
[IO.File]::WriteAllText("$r0\r3_authored.toe.dir\project1\authored_v2.n", $node, [Text.UTF8Encoding]::new($false))

$lines = [IO.File]::ReadAllLines("$r0\r3_authored.toe.toc")          # strips CR
$text = ($lines -join "`n") + "`n"
[IO.File]::WriteAllText("$r0\r3_authored.toe.toc", $text, [Text.UTF8Encoding]::new($false))
# insert entry (position tolerated by TD; keep near siblings for readability)
$all = [IO.File]::ReadAllLines("$r0\r3_authored.toe.toc")
$i = [Array]::IndexOf($all, "project1.panel")
if ($i -lt 0) { $all = @($all[0..20]) + "project1/authored_v2.n" + @($all[21..($all.Count-1)]) } else { $all = $all[0..$i] + "project1/authored_v2.n" + $all[($i+1)..($all.Count-1)] }
[IO.File]::WriteAllText("$r0\r3_authored.toe.toc", (($all -join "`n") + "`n"), [Text.UTF8Encoding]::new($false))

Remove-Item "$r0\r3_authored.toe" -Force -ErrorAction SilentlyContinue
& "$bin\toecollapse.exe" "$r0\r3_authored.toe" | Out-String | Write-Host
$packed = Get-Item "$r0\r3_authored.toe" -ErrorAction SilentlyContinue
if (-not $packed -or $packed.Length -eq 0) { "PACK FAILED (empty/missing output, exit lies)"; exit 1 }
"packed ok: $($packed.Length) bytes -> now run r2-spawn-lifecycle.ps1 on it, then inspect /project1/authored_v2 via the daemon"
