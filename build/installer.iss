; Inno Setup script for Flooring Partners App (QR codes and the VendorCafe
; CSV converter in one program).
; Build the exe first (cargo build --release), then from the repo root:
;   iscc build\installer.iss
; Requires Inno Setup 6.3+ (7.x works too).

#define AppName        "Flooring Partners App"
; The name used up to 3.0.1, whose shortcuts are replaced on upgrade.
#define OldAppName     "Flooring Partners Tools"
; CI passes /DAppVersion=x.y.z from Cargo.toml; the fallback keeps local builds working.
#ifndef AppVersion
  #define AppVersion   "3.0.0"
#endif
#define AppPublisher   "Flooring Partners"
#define AppExeName     "FPTools.exe"
; Inno Setup AppIds of the separate apps this one replaces.
#define OldQrAppId     "{0234F8BE-94D5-4A96-8387-A0CB5A5DAD1B}"
#define OldVcAppId     "{AB4F23FC-7020-4D60-9CFC-3EB453F9A9BA}"

[Setup]
; AppId is the identity Windows uses to recognise an upgrade. NEVER change it
; between releases, or every version installs side by side instead of replacing
; the previous one. New for the combined program: it must not match the old
; QR Code Generator's or VendorCafe CSV Converter's, which are separate
; products that this one removes (see [Code]).
AppId={{87BB602F-B003-4EE3-9EB5-6B103697BEB2}

AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}

; CI defines Sign (and passes /Sfpsign=...) when a certificate is configured,
; so the uninstaller is signed as well as Setup itself.
#ifdef Sign
SignTool=fpsign
SignedUninstaller=yes
#endif

; Machine-wide by default: one copy in Program Files for every user, which
; only administrators can modify. "Install for me only" stays available
; (dialog, or /CURRENTUSER on the command line), and /ALLUSERS forces the
; machine-wide mode for unattended deployment.
;
; UsePreviousPrivileges (default yes) means an existing install is upgraded in
; the mode it was installed in, so the in-app updater keeps a per-user copy
; per-user, with no UAC prompt.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
DefaultDirName={autopf}\{#AppPublisher}\Tools
DisableProgramGroupPage=yes

OutputDir=..\dist\installer
OutputBaseFilename=FPTools-{#AppVersion}-setup
SetupIconFile=..\assets\fp.ico
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppName}

Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; The program is a 64-bit Windows 10+ executable.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 version 1607 or later (per-monitor DPI functions).
MinVersion=10.0.14393

; Shut down running copies so the exe can be replaced instead of demanding a
; reboot. On a shared machine (Remote Desktop / terminal server) an
; administrator's update closes every user's copy; such sites should turn the
; in-app updater off (see /DISABLEUPDATES below) and deploy centrally.
CloseApplications=yes
RestartApplications=no
CloseApplicationsFilter={#AppExeName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"
; Only offered when installing for all users and an old app is installed for
; all users. Copies each person installed for themselves are found and offered
; for removal by the program itself, after it has copied their settings.
; Unattended installs that must keep the old apps: /MERGETASKS="!removeoldapps"
Name: "removeoldapps"; Description: "Remove the separate QR Code Generator and VendorCafe CSV Converter apps (installed for all users)"; GroupDescription: "Replaced programs:"; Check: OldAppsForAllUsers

[InstallDelete]
; Up to 3.0.1 this was "Flooring Partners Tools": remove those shortcuts so an
; upgrade doesn't leave two. (Taskbar pins point at the exe, whose name is
; unchanged, and keep working.)
Type: files; Name: "{autoprograms}\{#OldAppName}.lnk"
Type: files; Name: "{autodesktop}\{#OldAppName}.lnk"

[Files]
; One self-contained executable. No DLLs are shipped.
Source: "..\target\release\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[INI]
; policy.ini tells the program how it was installed and holds administrator
; settings (see README). Inno edits individual keys, so values IT has added
; by hand survive upgrades.
Filename: "{app}\policy.ini"; Section: "Install"; Key: "Scope"; String: "{code:InstallScope}"
Filename: "{app}\policy.ini"; Section: "Updates"; Key: "Enabled"; String: "0"; Check: HasParam('/DISABLEUPDATES')
Filename: "{app}\policy.ini"; Section: "Updates"; Key: "Enabled"; String: "1"; Check: HasParam('/ENABLEUPDATES')

[UninstallDelete]
Type: files; Name: "{app}\policy.ini"
; Per-user settings (%LOCALAPPDATA%\Flooring Partners\Tools) are
; deliberately left alone: an uninstall runs as one user and cannot
; reach the other users' profiles, and deleting only the administrator's own
; settings would be arbitrary.

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
; Interactive install: a checkbox on the Finished page. postinstall entries
; run as the user who started Setup, not the elevated administrator.
Filename: "{app}\{#AppExeName}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent

; Self-update (/SILENT from the in-app updater): reopen the program for the
; user who was using it. runasoriginaluser matters for machine-wide installs:
; without it the program would come back running elevated, as whichever
; administrator approved the UAC prompt.
Filename: "{app}\{#AppExeName}"; Flags: nowait runasoriginaluser; Check: ShouldRelaunch

[Code]
const
  { "Shell Folders" holds already-expanded paths, so redirected or OneDrive-
    backed locations resolve correctly. Reading the profile path and appending
    "Favorites" would miss those. }
  ShellFoldersKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders';

function ShellFolder(const ValueName: String): String;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, ShellFoldersKey, ValueName, Result) then
    Result := '';
end;

function IsUnder(const Child, Parent: String): Boolean;
var
  P: String;
begin
  Result := False;
  if Parent = '' then
    Exit;
  P := AddBackslash(Lowercase(Parent));
  Result := Copy(AddBackslash(Lowercase(Child)), 1, Length(P)) = P;
end;

{ A machine-wide install must live where only administrators can write.
  Program Files is. A folder such as C:\Tools inherits "Users: Modify" from
  the drive root, so any user could replace the program (or its policy.ini)
  for everyone. Silent installs are not checked: there IT chose /DIR=. }
function IsAdminOnlyFolder(const Dir: String): Boolean;
begin
  Result := IsUnder(Dir, ExpandConstant('{commonpf64}')) or IsUnder(Dir, ExpandConstant('{commonpf32}'));
end;

function AdminFolderAcceptable(const Dir: String): Boolean;
begin
  Result := True;
  if IsAdminInstallMode and not IsAdminOnlyFolder(Dir) then
  begin
    MsgBox('For an install for all users, choose a folder inside ' + ExpandConstant('{commonpf64}') + '.'
      + #13#10#13#10
      + 'Other folders can usually be changed by any user of this computer, who could then '
      + 'replace the program for everyone.',
      mbError, MB_OK);
    Result := False;
  end;
end;

{ Returns the friendly name of the protected folder Dir sits inside, or an
  empty string. These are Microsoft Defender's default Controlled folder access
  locations. Installing into one makes the program an untrusted binary living
  inside the area Defender guards: SmartScreen fires repeatedly, the program
  cannot write files, and the uninstaller cannot remove its own files. }
function ProtectedFolderName(const Dir: String): String;
begin
  Result := '';
  if IsUnder(Dir, ShellFolder('Personal')) then
    Result := 'Documents'
  else if IsUnder(Dir, ShellFolder('My Pictures')) then
    Result := 'Pictures'
  else if IsUnder(Dir, ShellFolder('My Music')) then
    Result := 'Music'
  else if IsUnder(Dir, ShellFolder('My Video')) then
    Result := 'Videos'
  else if IsUnder(Dir, ShellFolder('Desktop')) then
    Result := 'Desktop'
  else if IsUnder(Dir, ShellFolder('Favorites')) then
    Result := 'Favorites';
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  FolderName: String;
begin
  Result := True;

  { WizardSilent is excluded deliberately. NextButtonClick also runs on silent
    installs, and returning False there makes Setup exit with no message - which
    would break the self-updater for anyone who already has a bad install. }
  if (CurPageID = wpSelectDir) and not WizardSilent then
  begin
    if not AdminFolderAcceptable(WizardDirValue) then
    begin
      Result := False;
      Exit;
    end;
    FolderName := ProtectedFolderName(WizardDirValue);
    if FolderName <> '' then
    begin
      MsgBox('This program cannot be installed inside your ' + FolderName + ' folder.'
        + #13#10#13#10
        + 'Windows ransomware protection guards that folder, which would stop the '
        + 'program saving files, cause repeated security warnings, and prevent it '
        + 'from being uninstalled afterwards.'
        + #13#10#13#10
        + 'Please choose a different folder, or click Back and accept the default.',
        mbError, MB_OK);
      Result := False;
    end;
  end;
end;

function HasParam(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
    begin
      Result := True;
      Exit;
    end;
end;

{ ---- The separate apps this program replaces ---- }

function OldAppKey(const AppId: String): String;
begin
  Result := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' + AppId + '_is1';
end;

{ The uninstaller named by an old app's all-users registration, if it looks
  like an Inno Setup uninstaller: unins###.exe that exists. HKLM is writable
  only by administrators, so this is a sanity check, not a trust boundary. }
function OldUninstaller(const RootKey: Integer; const AppId: String): String;
var
  Value, Name: String;
begin
  Result := '';
  if not RegQueryStringValue(RootKey, OldAppKey(AppId), 'UninstallString', Value) then
    Exit;
  Value := RemoveQuotes(Trim(Value));
  Name := Lowercase(ExtractFileName(Value));
  if (Length(Name) = 12) and (Copy(Name, 1, 5) = 'unins') and (Copy(Name, 9, 4) = '.exe') and FileExists(Value) then
    Result := Value;
end;

function OldAppsForAllUsers: Boolean;
begin
  Result := IsAdminInstallMode and (
    (OldUninstaller(HKLM64, '{#OldQrAppId}') <> '') or (OldUninstaller(HKLM32, '{#OldQrAppId}') <> '') or
    (OldUninstaller(HKLM64, '{#OldVcAppId}') <> '') or (OldUninstaller(HKLM32, '{#OldVcAppId}') <> ''));
end;

procedure RemoveOldApp(const RootKey: Integer; const AppId: String);
var
  Uninstaller: String;
  ResultCode: Integer;
begin
  Uninstaller := OldUninstaller(RootKey, AppId);
  if Uninstaller = '' then
    Exit;
  Log('Removing replaced program: ' + Uninstaller);
  if not Exec(Uninstaller, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Log('Could not start it: ' + SysErrorMessage(ResultCode))
  else if ResultCode <> 0 then
    Log('It exited with code ' + IntToStr(ResultCode));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  { After the new program is in place, so nobody is left without either. }
  if (CurStep = ssPostInstall) and IsAdminInstallMode and WizardIsTaskSelected('removeoldapps') then
  begin
    RemoveOldApp(HKLM64, '{#OldQrAppId}');
    RemoveOldApp(HKLM32, '{#OldQrAppId}');
    RemoveOldApp(HKLM64, '{#OldVcAppId}');
    RemoveOldApp(HKLM32, '{#OldVcAppId}');
  end;
end;

function InstallScope(Param: String): String;
begin
  if IsAdminInstallMode then
    Result := 'machine'
  else
    Result := 'user';
end;

{ Relaunch only for the in-app updater's /SILENT run. Never for /VERYSILENT
  (unattended deployment), /NORELAUNCH, or when Setup runs as the SYSTEM
  account (RMM / Intune / GPO), which has no user to show the program to. }
function ShouldRelaunch: Boolean;
begin
  Result := WizardSilent
    and not HasParam('/VERYSILENT')
    and not HasParam('/NORELAUNCH')
    and (CompareText(GetUserNameString, 'SYSTEM') <> 0);
end;
