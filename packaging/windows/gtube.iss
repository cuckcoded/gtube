; Inno Setup script for GTube's Windows installer, made from the folder
; packaging/windows/package.sh builds:
;   iscc /DAppVersion=0.4.0 packaging\windows\gtube.iss
; Writes build\GTube-setup-x86_64.exe.

#ifndef AppVersion
  #define AppVersion "0.4.0"
#endif

[Setup]
AppId={{8CFD2AC0-5AFC-442F-A2D6-3F5D74F62E29}
AppName=GTube
AppVersion={#AppVersion}
AppVerName=GTube {#AppVersion}
AppPublisher=GTube
DefaultDirName={autopf}\GTube
DefaultGroupName=GTube
DisableProgramGroupPage=yes
; Installs for the current user unless the user picks "all users".
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\build
OutputBaseFilename=GTube-setup-x86_64
SetupIconFile=..\..\build\GTube\gtube.ico
UninstallDisplayIcon={app}\gtube.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\build\GTube\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\GTube"; Filename: "{app}\bin\gtube.exe"; IconFilename: "{app}\gtube.ico"
Name: "{autodesktop}\GTube"; Filename: "{app}\bin\gtube.exe"; IconFilename: "{app}\gtube.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\bin\gtube.exe"; Description: "{cm:LaunchProgram,GTube}"; Flags: nowait postinstall skipifsilent
