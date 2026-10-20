; Inno Setup script for Hyperdesk (built by scripts\package-windows.ps1, which passes
; AppVersion, SourceDir and OutputDir). Installs per user, so no administrator rights are
; needed; settings, layouts and caches live in %APPDATA%\Hyperdesk and are kept on uninstall.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\hyperdesk"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{6F1C3A52-8E0B-4C51-9B7A-2D5E7C1F4A90}
AppName=Hyperdesk
AppVersion={#AppVersion}
AppPublisher=Hyperdesk
DefaultDirName={localappdata}\Programs\hyperdesk
DefaultGroupName=Hyperdesk
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=hyperdesk-{#AppVersion}-setup-x64
SetupIconFile=..\..\app\assets\icon.ico
UninstallDisplayIcon={app}\hyperdesk.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\Hyperdesk"; Filename: "{app}\hyperdesk.exe"
Name: "{userdesktop}\Hyperdesk"; Filename: "{app}\hyperdesk.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\hyperdesk.exe"; Description: "{cm:LaunchProgram,Hyperdesk}"; Flags: nowait postinstall skipifsilent
