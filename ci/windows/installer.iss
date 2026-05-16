; ABOUTME: Inno Setup script that wraps the release doombuilder.exe into a
; ABOUTME: standard Windows installer with Start Menu and optional desktop icon.

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{8F2C1A6E-7B4D-4C9A-9E33-DOOMBUILDER01}
AppName=DoomBuilder
AppVersion={#MyAppVersion}
AppPublisher=jsh
DefaultDirName={autopf}\DoomBuilder
DefaultGroupName=DoomBuilder
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=DoomBuilder-{#MyAppVersion}-setup
SetupIconFile=doombuilder.ico
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional icons:"

[Files]
Source: "{#SourceDir}\doombuilder.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\DoomBuilder"; Filename: "{app}\doombuilder.exe"
Name: "{group}\Uninstall DoomBuilder"; Filename: "{uninstallexe}"
Name: "{autodesktop}\DoomBuilder"; Filename: "{app}\doombuilder.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\doombuilder.exe"; Description: "Launch DoomBuilder"; Flags: nowait postinstall skipifsilent
