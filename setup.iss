; LocalChat — instalador (Inno Setup 6)
; Compilar:  "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" setup.iss
; Saida:     dist\LocalChat-Setup.exe  (use scripts\build-windows.ps1)

#define AppName "LocalChat"
; o CI passa /DAppVersion=x.y.z (lido do Cargo.toml)
#ifndef AppVersion
#define AppVersion "0.2.0"
#endif

[Setup]
AppId={{7B41D9E2-3A58-4C0F-B1D2-6E8A9C0F4B31}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=localchat
DefaultDirName={autopf}\LocalChat
DefaultGroupName={#AppName}
PrivilegesRequired=admin
OutputDir=dist
; nome fixo: o link "baixar" (releases/latest/download/LocalChat-Setup.exe) nunca muda
OutputBaseFilename=LocalChat-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\localchat.exe

[Files]
Source: "target\x86_64-pc-windows-gnu\release\localchat.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

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
; apaga antes de criar: cada update roda o instalador de novo e duplicaria a regra
Filename: "netsh"; Parameters: "advfirewall firewall delete rule name=""LocalChat"""; Flags: runhidden
Filename: "netsh"; Parameters: "advfirewall firewall add rule name=""LocalChat"" dir=in action=allow program=""{app}\localchat.exe"" enable=yes profile=private"; StatusMsg: "Configurando o firewall..."; Flags: runhidden
Filename: "{app}\localchat.exe"; Parameters: "gui"; Description: "Abrir o LocalChat agora"; Flags: nowait postinstall skipifsilent
; o update automatico roda o instalador em silencio: reabre o app (sem admin)
Filename: "{app}\localchat.exe"; Parameters: "gui"; Flags: nowait runasoriginaluser; Check: WizardSilent

[UninstallRun]
Filename: "netsh"; Parameters: "advfirewall firewall delete rule name=""LocalChat"""; RunOnceId: "RemoveFirewallRule"; Flags: runhidden
