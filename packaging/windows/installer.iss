; Inno Setup 6 — td-mcp-rs per-user installer (no UAC).
; Build (from repo root):
;   ISCC.exe /DVersion=v0.1.4 packaging/windows/installer.iss
; Payload source: staging\tdmcp-daemon.exe (CI extracts the packaged zip there;
; for a local dry run: cargo run -p xtask -- package --out dist, then extract
; the Windows zip to staging\).

#define AppName "td-mcp-rs"
#define AppEx "tdmcp-daemon.exe"
#ifndef Version
#define Version "0.0.0-dev"
#endif
#if Pos("v", Version) == 1
  #define AppVer = Copy(Version, 2)
#else
  #define AppVer = Version
#endif

[Setup]
; Stable GUID: upgrades/supersede keyed on this across versions. Never change.
AppId={{8E7C1A42-5D93-4B6F-9A21-C3D4E5F60718}}
AppName={#AppName}
AppVersion={#AppVer}
AppPublisher=Verbalize
DefaultDirName={localappdata}\Programs\tdmcp-rs
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
OutputDir=..\..\dist
OutputBaseFilename=tdmcp-rs-{#AppVer}-x64-setup
SetupIconFile=app.ico
UninstallDisplayName={#AppName}
CloseApplications=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "..\..\staging\{#AppEx}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\staging\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppEx}"

[UninstallRun]
; The tray daemon must die before its exe can be removed.
Filename: "{cmd}"; Parameters: "/C taskkill /IM {#AppEx} /F >nul 2>&1"; Flags: runhidden; RunOnceId: "KillDaemon"

[Code]
// Inno's Restart Manager rarely sees a windowless tray app; force-stop it
// before files are copied. Errors ignored (nothing running = fine).
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{cmd}'),
    Format('/C taskkill /IM %s /F >nul 2>&1', ['{#AppEx}']),
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := '';
end;
