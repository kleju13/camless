; Builds camless-setup.exe, which installs camless.exe and camless-gui.exe in Program Files.
; scripts\package.ps1 builds both exes and runs this with the version:
;   ISCC /DAppVersion=0.5.0-beta.1 /DAppNumericVersion=0.5.0 installer\camless.iss
;
; The setup asks for administrator rights. The service runs with them, so camless must be where
; only administrators write: in a folder of the user's, any program they run could replace it.
; Until 0.6.0-beta.6 the setup put it in %LOCALAPPDATA%\Programs\camless; this one moves it.
;
; Running it over an installed camless updates it in place: it closes the app, stops the service
; through its loopback API, replaces both exes and opens the app again, which starts the service.
; /VERYSILENT does the same without a window. Settings and profiles in %APPDATA%\camless stay.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=x.y.z camless.iss
#endif
; The file version takes numbers only; a beta's is its version without the suffix.
#ifndef AppNumericVersion
  #if Pos("-", AppVersion) > 0 || Pos("+", AppVersion) > 0
    #error Pass the number part of a pre-release too: ISCC /DAppVersion=x.y.z-beta.n /DAppNumericVersion=x.y.z camless.iss
  #endif
  #define AppNumericVersion AppVersion
#endif

[Setup]
AppId={{7D97C062-B088-4D5D-A5BE-24FFC0DC4483}
AppName=camless
AppVersion={#AppVersion}
AppVerName=camless {#AppVersion}
AppPublisher=kleju13
AppPublisherURL=https://github.com/kleju13/camless
AppSupportURL=https://github.com/kleju13/camless/issues
AppUpdatesURL=https://github.com/kleju13/camless/releases
VersionInfoVersion={#AppNumericVersion}
DefaultDirName={autopf}\camless
; Always Program Files, also over a copy that /DIR once put elsewhere.
UsePreviousAppDir=no
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
; The setup starts without them and asks Windows itself, so the app of an older camless, which
; starts it as a plain program, still updates.
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=..\gui\camless.ico
UninstallDisplayIcon={app}\camless-gui.exe
UninstallDisplayName=camless
; The service runs elevated, so the Restart Manager could not close it. StopCamless below does.
CloseApplications=no
OutputDir=..\dist
OutputBaseFilename=camless-setup
Compression=lzma2/max
SolidCompression=yes

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\camless.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\camless-gui.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\camless"; Filename: "{app}\camless-gui.exe"; Comment: "Fan, pump, lighting and LCD control for NZXT devices"
Name: "{autodesktop}\camless"; Filename: "{app}\camless-gui.exe"; Check: DesktopShortcut

[Run]
; Not skipped when silent, so an update opens the app again, and the app starts the service.
; The app runs as the user who started the setup, without the setup's administrator rights.
Filename: "{app}\camless-gui.exe"; Description: "Open camless"; Flags: postinstall nowait runasoriginaluser

[UninstallDelete]
; The PawnIO modules the user put next to camless.exe.
Type: files; Name: "{app}\IntelMSR.bin"
Type: files; Name: "{app}\AMDFamily17.bin"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  { Where the setup listed camless for one user, until 0.6.0-beta.6. }
  OldUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{7D97C062-B088-4D5D-A5BE-24FFC0DC4483}_is1';

var
  { The folder of a camless installed for one user, which this setup replaces, or empty. }
  OldDir: String;
  { That camless had a shortcut on the user's desktop. }
  HadDesktopShortcut: Boolean;

function DesktopShortcut: Boolean;
begin
  Result := WizardIsTaskSelected('desktopicon') or HadDesktopShortcut;
end;

{ The service's API address: listen under [api] in camless.toml, which is next to the exe if there
  is one, also the exe of a camless this setup replaces, else in %APPDATA%\camless. }
function ApiAddress: String;
var
  Lines: TArrayOfString;
  Path, Line: String;
  I, Q: Integer;
  InApi: Boolean;
begin
  Result := '127.0.0.1:30641';
  Path := ExpandConstant('{app}\camless.toml');
  if not FileExists(Path) and (OldDir <> '') then
    Path := OldDir + '\camless.toml';
  if not FileExists(Path) then
    Path := ExpandConstant('{userappdata}\camless\camless.toml');
  if not LoadStringsFromFile(Path, Lines) then
    Exit;
  InApi := False;
  for I := 0 to GetArrayLength(Lines) - 1 do
  begin
    Line := Trim(Lines[I]);
    if Copy(Line, 1, 1) = '[' then
      InApi := Line = '[api]'
    else if InApi and (Copy(Line, 1, 6) = 'listen') then
    begin
      Q := Pos('"', Line);
      if Q > 0 then
      begin
        Line := Copy(Line, Q + 1, Length(Line));
        Q := Pos('"', Line);
        if Q > 1 then
          Result := Copy(Line, 1, Q - 1);
      end;
    end;
  end;
end;

{ Sends a request to the service's API and returns the HTTP status, or 0 if nothing answers. }
function Request(const Verb, Url: String): Integer;
var
  Http: Variant;
begin
  Result := 0;
  try
    Http := CreateOleObject('WinHttp.WinHttpRequest.5.1');
    Http.SetProxy(1);
    Http.SetTimeouts(2000, 2000, 2000, 2000);
    Http.Open(Verb, Url, False);
    if Verb = 'POST' then
    begin
      Http.SetRequestHeader('Content-Type', 'application/json');
      Http.Send('{}');
    end else
      Http.Send('');
    Result := Http.Status;
  except
  end;
end;

{ Closes the app and stops the service, so that their exes can be replaced or removed. This setup
  cannot end the elevated service, but POST /v1/stop does, as Ctrl+C would: host-driven fans go to
  the failsafe duty and the LCD back to its own screen. The API answers until the process exits. }
function StopCamless: Boolean;
var
  Url: String;
  Code, I: Integer;
begin
  Result := True;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM camless-gui.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
  Url := 'http://' + ApiAddress + '/v1/';
  if Request('POST', Url + 'stop') <> 202 then
    Exit;
  for I := 1 to 60 do
  begin
    Sleep(250);
    if Request('GET', Url + 'status') = 0 then
    begin
      Sleep(1000);
      Exit;
    end;
  end;
  Result := False;
end;

function TaskName: String;
begin
  Result := '"camless (' + GetUserNameString + ')"';
end;

{ Whether this user has the task the app sets up for Start as administrator without asking. }
function TaskExists: Boolean;
var
  Code: Integer;
begin
  Result := Exec(ExpandConstant('{sys}\schtasks.exe'), '/Query /TN ' + TaskName, '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

function RemoveTask: Boolean;
var
  Code: Integer;
begin
  Result := Exec(ExpandConstant('{sys}\schtasks.exe'), '/Delete /TN ' + TaskName + ' /F', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
end;

{ Whether the folder or file at Path is a link, such as a junction. }
function IsLink(const Path: String): Boolean;
var
  Find: TFindRec;
begin
  Result := FindFirst(Path, Find);
  if not Result then
    Exit;
  Result := Find.Attributes and FILE_ATTRIBUTE_REPARSE_POINT <> 0;
  FindClose(Find);
end;

{ Until 0.6.0-beta.6 the setup installed camless for one user, in a folder of theirs. This finds
  that copy, before the service is stopped and the new one's shortcuts are made. A folder that
  is a link is left alone: it could lead to another program's folder. }
procedure FindOldInstall;
var
  Programs: String;
begin
  Programs := ExpandConstant('{localappdata}\Programs');
  OldDir := Programs + '\camless';
  if (CompareText(OldDir, WizardDirValue) = 0) or not FileExists(OldDir + '\camless-gui.exe')
      or IsLink(Programs) or IsLink(OldDir) then
    OldDir := ''
  else
    HadDesktopShortcut := FileExists(ExpandConstant('{userdesktop}\camless.lnk'));
end;

function Quoted(const Path: String): String;
begin
  Result := ' "' + Path + '"';
end;

{ Removes that copy once the new one is in place: its two exes, its uninstaller, its shortcuts,
  a PawnIO module that the new copy has now, and its entry in Windows' list of apps. The folder
  goes too if nothing else is left in it. Nothing from the folder is run, since this setup has
  administrator rights and any program of the user's could have replaced what is there. For the
  same reason the files are deleted with the user's rights, not the setup's: the folder could
  become a link to another one between the look at it and the delete. The app repairs Start with
  Windows by itself. }
procedure RemoveOldInstall;
var
  Files: String;
  Code: Integer;
begin
  Files := Quoted(ExpandConstant('{userdesktop}\camless.lnk')) + Quoted(ExpandConstant('{userprograms}\camless.lnk'))
    + Quoted(OldDir + '\camless.exe') + Quoted(OldDir + '\camless-gui.exe') + Quoted(OldDir + '\unins???.*');
  if FileExists(ExpandConstant('{app}\IntelMSR.bin')) then
    Files := Files + Quoted(OldDir + '\IntelMSR.bin');
  if FileExists(ExpandConstant('{app}\AMDFamily17.bin')) then
    Files := Files + Quoted(OldDir + '\AMDFamily17.bin');
  ExecAsOriginalUser(ExpandConstant('{cmd}'), '/C del /F /Q' + Files + ' & rd' + Quoted(OldDir), '', SW_HIDE, ewWaitUntilTerminated, Code);
  RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, OldUninstallKey);
end;

{ Copies a PawnIO module the user put next to the old camless.exe to the new one. Only a plain
  file of a module's size: a link there could name a file the user may not read. }
procedure CopyModule(const Name: String);
var
  Find: TFindRec;
  Plain: Boolean;
  Old, Target: String;
begin
  Old := OldDir + '\' + Name;
  Target := ExpandConstant('{app}\') + Name;
  if not FindFirst(Old, Find) then
    Exit;
  Plain := (Find.Attributes and (FILE_ATTRIBUTE_DIRECTORY or FILE_ATTRIBUTE_REPARSE_POINT) = 0)
    and (Find.SizeHigh = 0) and (Find.SizeLow < 1048576);
  FindClose(Find);
  if Plain and not FileExists(Target) then
    CopyFile(Old, Target, True);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  { First, for the address of a service that reads a camless.toml beside the old exe. }
  FindOldInstall;
  if not StopCamless then
    Result := 'The camless service did not stop. Restart Windows, then run setup again.';
end;

{ The task names the exe it starts. The app writes it again for the camless just installed: one
  left pointing at the old folder would start, as administrator, whatever is put there. So a
  task that cannot be written again is deleted, and the app asks through UAC as before. }
procedure RenewTask;
var
  Code: Integer;
begin
  if not TaskExists then
    Exit;
  if Exec(ExpandConstant('{app}\camless-gui.exe'), '--set-up-task', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0) then
    Exit;
  if RemoveTask then
    Log('The task for Start as administrator without asking could not be written again, so it was deleted.')
  else
    Log('The task for Start as administrator without asking could not be written again or deleted.');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep <> ssPostInstall then
    Exit;
  RenewTask;
  if OldDir <> '' then
  begin
    CopyModule('IntelMSR.bin');
    CopyModule('AMDFamily17.bin');
    RemoveOldInstall;
  end;
end;

function InitializeUninstall: Boolean;
begin
  Result := StopCamless;
  if not Result then
    MsgBox('The camless service did not stop. Restart Windows, then uninstall again.', mbError, MB_OK);
end;

{ Deletes the task the app sets up for Start as administrator without asking. A task left behind
  would start nothing, but a failure is reported all the same. }
procedure DeleteTask;
begin
  if not TaskExists or RemoveTask then
    Exit;
  if not UninstallSilent then
    MsgBox('camless could not delete its scheduled task ' + TaskName + '. Delete it in Task Scheduler.', mbError, MB_OK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  { The app adds this value for Start with Windows. }
  RegDeleteValue(HKEY_CURRENT_USER, RunKey, 'camless');
  DeleteTask;
  if not UninstallSilent and (MsgBox('Also delete your camless settings, profiles and history?' + #13#10#13#10
      + ExpandConstant('{userappdata}\camless') + #13#10 + ExpandConstant('{localappdata}\camless')
      + #13#10 + ExpandConstant('{app}\data'),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
  begin
    DelTree(ExpandConstant('{userappdata}\camless'), True, True, True);
    DelTree(ExpandConstant('{localappdata}\camless'), True, True, True);
    { The service's own folder for the history, while it runs as administrator. }
    DelTree(ExpandConstant('{app}\data'), True, True, True);
    RemoveDir(ExpandConstant('{app}'));
  end;
end;
