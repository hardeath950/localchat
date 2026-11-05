; LocalChat — instalador (Inno Setup 6)
; Compilar:  "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" setup.iss
; Saida:     dist\LocalChat-Setup-0.1.0.exe

#define AppName "LocalChat"
#define AppVersion "0.1.0"

[Setup]
AppId={{7B41D9E2-3A58-4C0F-B1D2-6E8A9C0F4B31}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=localchat
DefaultDirName={autopf}\LocalChat
DefaultGroupName={#AppName}
PrivilegesRequired=admin
OutputDir=dist
OutputBaseFilename=LocalChat-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\localchat.exe

[Files]
Source: "target\x86_64-pc-windows-gnu\release\localchat.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.md"; DestDir: "{app}"; Flags: ignoreversion

[Tasks]
Name: "desktopicon"; Description: "Atalho na area de trabalho"
Name: "startup"; Description: "Iniciar o LocalChat automaticamente com o Windows"; Flags: unchecked

[Icons]
Name: "{group}\LocalChat"; Filename: "{app}\localchat.exe"; Parameters: "gui"; Comment: "Chat e transferencia de arquivos na rede local"
Name: "{group}\README"; Filename: "{app}\README.md"
Name: "{autodesktop}\LocalChat"; Filename: "{app}\localchat.exe"; Parameters: "gui"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "LocalChat"; ValueData: """{app}\localchat.exe"" gui"; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "netsh"; Parameters: "advfirewall firewall add rule name=""LocalChat"" dir=in action=allow program=""{app}\localchat.exe"" enable=yes profile=private"; StatusMsg: "Configurando o firewall..."; Flags: runhidden
Filename: "{app}\localchat.exe"; Parameters: "gui"; Description: "Abrir o LocalChat agora"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "netsh"; Parameters: "advfirewall firewall delete rule name=""LocalChat"""; RunOnceId: "RemoveFirewallRule"; Flags: runhidden
