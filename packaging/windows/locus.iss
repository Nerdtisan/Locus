; Inno Setup script for Locus (SPEC §0 Packaging, #245).
; Compile: iscc /DAppVersion=1.2.3 /DSourceDir=<dir with locus.exe, locus-cli.exe> /DOutDir=<out> locus.iss
; Per-user install (no admin rights): files go to %LOCALAPPDATA%\Programs\Locus and
; file associations are written under HKCU, so uninstalling removes everything.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<folder with the binaries>
#endif
#ifndef OutDir
  #define OutDir "."
#endif

[Setup]
; Stable GUID: identifies the app across upgrades. Never change it.
AppId={{6B0F7D9E-3C1A-4E52-9A47-4C0C5D2B8E11}
AppName=Locus
AppVersion={#AppVersion}
AppPublisher=Locus contributors
AppPublisherURL=https://github.com/Nerdtisan/Locus
DefaultDirName={autopf}\Locus
DefaultGroupName=Locus
PrivilegesRequired=lowest
OutputDir={#OutDir}
OutputBaseFilename=Locus-{#AppVersion}-windows-setup
SetupIconFile=..\icon\locus.ico
UninstallDisplayIcon={app}\locus.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Tells Explorer to refresh associations after install/uninstall.
ChangesAssociations=yes
; The licence is AGPL-3.0-or-later (SPEC §2).
LicenseFile=..\..\LICENSE
; SIGNING (stub): the release workflow signs the output with signtool after this
; step once WINDOWS_CERT_PFX_BASE64 exists; nothing to configure here.

[Files]
Source: "{#SourceDir}\locus.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\locus-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\icon\locus.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Locus"; Filename: "{app}\locus.exe"; IconFilename: "{app}\locus.ico"
Name: "{group}\Uninstall Locus"; Filename: "{uninstallexe}"

[Registry]
; .locus documents. Written under HKCU\Software\Classes, which is what a per-user
; install must use. uninsdeletekey removes the keys on uninstall.
Root: HKA; Subkey: "Software\Classes\.locus"; ValueType: string; ValueData: "Locus.Document"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Locus.Document"; ValueType: string; ValueData: "Locus document"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Locus.Document\DefaultIcon"; ValueType: string; ValueData: "{app}\locus.ico"
Root: HKA; Subkey: "Software\Classes\Locus.Document\shell\open\command"; ValueType: string; ValueData: """{app}\locus.exe"" ""%1"""
; .locusplugin archives. The app does not install plugins from a double-click yet
; (it ignores the path); association is registered so the file type is recognised.
Root: HKA; Subkey: "Software\Classes\.locusplugin"; ValueType: string; ValueData: "Locus.Plugin"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Locus.Plugin"; ValueType: string; ValueData: "Locus plugin"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Locus.Plugin\DefaultIcon"; ValueType: string; ValueData: "{app}\locus.ico"
Root: HKA; Subkey: "Software\Classes\Locus.Plugin\shell\open\command"; ValueType: string; ValueData: """{app}\locus.exe"" ""%1"""

[Run]
Filename: "{app}\locus.exe"; Description: "Launch Locus"; Flags: nowait postinstall skipifsilent
