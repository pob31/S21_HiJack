; ----------------------------------------------------------------------
; S21 HiJack — Inno Setup installer
;
; Builds a distributable Windows installer for s21_hijack.exe, bundling the
; runtime `locales\` tree (required for the help-bubble translations — the app
; scans for `locales\` beside its executable; see src/ui/help.rs::locale_dirs),
; the README, and both license files, and registering the .s21show file
; association.
;
; Build it with the release helper (reads the version from Cargo.toml):
;     scripts\build-windows-release.ps1
; or directly:
;     ISCC /DMyAppVersion=0.1.0 packaging\windows\s21_hijack.iss
;
; Requires the release exe at target\release\s21_hijack.exe — run
; `cargo build --release --bin s21_hijack` first (the helper does this).
; Paths below are relative to this .iss file (packaging\windows\).
; ----------------------------------------------------------------------

; Version: overridable on the ISCC command line (/DMyAppVersion=x.y.z). The
; default keeps a standalone `ISCC s21_hijack.iss` working; the build helper
; passes the Cargo.toml version so the installer matches the GitHub release tag
; the in-app update check compares against.
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#define MyAppName "S21 HiJack"
#define MyAppPublisher "pob31"
#define MyAppExe "s21_hijack.exe"
#define MyAppUrl "https://github.com/pob31/S21_HiJack"

[Setup]
; Stable application id — never change this once shipped (drives upgrade
; detection and the uninstall entry).
AppId={{90D082E1-C5AC-4750-A8A3-443E195B70BD}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppUrl}
AppSupportURL={#MyAppUrl}
AppUpdatesURL={#MyAppUrl}/releases
VersionInfoVersion={#MyAppVersion}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#MyAppExe}
LicenseFile=..\..\LICENSE-MIT
OutputDir=..\..\dist
OutputBaseFilename=s21_hijack-v{#MyAppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\icon.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; 64-bit only build. Requires Inno Setup 6.3+ for the `x64compatible`
; identifier; on older 6.x use `x64` instead.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Let the operator install per-user (no admin) or per-machine; the HKA
; registry root in the association fragment follows this choice.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\release\{#MyAppExe}"; DestDir: "{app}"; Flags: ignoreversion
; Runtime translations — MUST sit beside the exe or every tooltip falls back
; to English. Ships the whole locales tree (template.json is ignored at load).
Source: "..\..\assets\locales\*"; DestDir: "{app}\locales"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExe}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

; ── .s21show file association ──────────────────────────────────────────
; Shared with the standalone fragment so there's one source of truth for the
; registry entries (ProgID, DefaultIcon → exe,0, open command). HKA installs
; to HKLM under admin / HKCU otherwise, matching PrivilegesRequired above.
#include "..\..\assets\s21_hijack_assoc.iss"
