; Build with ISCC /DReleaseVersion=... /DAppExe=... /DLicensePath=... /O<output-dir>.
[Setup]
AppId={{67574C8E-53ED-41DF-B71F-F25280D7210D}
AppName=Corral
AppVersion={#ReleaseVersion}
AppPublisher=Corral
AppPublisherURL=https://github.com/pawissanutt/corral
DefaultDirName={localappdata}\Programs\Corral Desktop
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputBaseFilename=corral-app-windows-x86_64-setup
UninstallDisplayIcon={app}\Corral.exe
CloseApplications=yes
RestartApplications=no
LicenseFile={#LicensePath}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; DestName: "Corral.exe"; Flags: ignoreversion
Source: "{#LicensePath}"; DestDir: "{app}"; DestName: "LICENSE"; Flags: ignoreversion
Source: "..\assets\*"; DestDir: "{app}\assets\@nsh-app-corral"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{userprograms}\Corral"; Filename: "{app}\Corral.exe"; WorkingDir: "{app}"; AppUserModelID: "com.github.pawissanutt.corral"
