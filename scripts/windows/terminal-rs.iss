; Inno Setup script for terminal-rs (built by scripts\package-windows.ps1, which passes
; AppVersion, SourceDir and OutputDir). Installs per user, so no administrator rights are
; needed; settings, layouts and caches live in %APPDATA%\terminal-rs and are kept on uninstall.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\terminal-rs"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{6F1C3A52-8E0B-4C51-9B7A-2D5E7C1F4A90}
AppName=terminal-rs
AppVersion={#AppVersion}
AppPublisher=terminal-rs contributors
DefaultDirName={localappdata}\Programs\terminal-rs
DefaultGroupName=terminal-rs
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=terminal-rs-{#AppVersion}-setup-x64
SetupIconFile=..\..\app\assets\icon.ico
UninstallDisplayIcon={app}\terminal-rs.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\terminal-rs"; Filename: "{app}\terminal-rs.exe"
Name: "{userdesktop}\terminal-rs"; Filename: "{app}\terminal-rs.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\terminal-rs.exe"; Description: "{cm:LaunchProgram,terminal-rs}"; Flags: nowait postinstall skipifsilent
