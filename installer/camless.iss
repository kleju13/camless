; Builds camless-setup.exe, which installs camless.exe and camless-gui.exe for the current user,
; without administrator rights. scripts\package.ps1 builds both exes and runs this with the version:
;   ISCC /DAppVersion=0.5.0-beta.1 /DAppNumericVersion=0.5.0 installer\camless.iss
;
; Running it over an installed camless updates it in place: it closes the app, stops the service
; through its loopback API, replaces both exes and opens the app again, which starts the service.
; /VERYSILENT does the same without a window. Settings and profiles in %APPDATA%\camless stay.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=x.y.z camless.iss
#endif
; The file version takes numbers only; a beta's is its version without the suffix.
#ifndef AppNumericVersion
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
DefaultDirName={localappdata}\Programs\camless
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
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
Name: "{autodesktop}\camless"; Filename: "{app}\camless-gui.exe"; Tasks: desktopicon

[Run]
; Not skipped when silent, so an update opens the app again, and the app starts the service.
Filename: "{app}\camless-gui.exe"; Description: "Open camless"; Flags: postinstall nowait

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';

{ The service's API address: listen under [api] in camless.toml, which is next to the exe if there
  is one, else in %APPDATA%\camless. }
function ApiAddress: String;
var
  Lines: TArrayOfString;
  Path, Line: String;
  I, Q: Integer;
  InApi: Boolean;
begin
  Result := '127.0.0.1:30641';
  Path := ExpandConstant('{app}\camless.toml');
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

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not StopCamless then
    Result := 'The camless service did not stop. Restart Windows, then run setup again.';
end;

function InitializeUninstall: Boolean;
begin
  Result := StopCamless;
  if not Result then
    MsgBox('The camless service did not stop. Restart Windows, then uninstall again.', mbError, MB_OK);
end;

{ Deletes the task the app sets up for Start as administrator without asking. Its user may delete
  it; if that fails anyway, Windows is asked. A task left behind would run whatever exe is put in
  the folder it points at, so a failure is reported. }
procedure DeleteTask;
var
  Schtasks, Name: String;
  Code: Integer;
begin
  Schtasks := ExpandConstant('{sys}\schtasks.exe');
  Name := '"camless (' + GetUserNameString + ')"';
  if not Exec(Schtasks, '/Query /TN ' + Name, '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
    Exit;
  if Exec(Schtasks, '/Delete /TN ' + Name + ' /F', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0) then
    Exit;
  if ShellExec('runas', Schtasks, '/Delete /TN ' + Name + ' /F', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0) then
    Exit;
  if not UninstallSilent then
    MsgBox('camless could not delete its scheduled task ' + Name + '. Delete it in Task Scheduler.', mbError, MB_OK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  { The app adds this value for Start with Windows. }
  RegDeleteValue(HKEY_CURRENT_USER, RunKey, 'camless');
  DeleteTask;
  if not UninstallSilent and (MsgBox('Also delete your camless settings and profiles?' + #13#10#13#10
      + ExpandConstant('{userappdata}\camless') + #13#10 + ExpandConstant('{localappdata}\camless'),
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES) then
  begin
    DelTree(ExpandConstant('{userappdata}\camless'), True, True, True);
    DelTree(ExpandConstant('{localappdata}\camless'), True, True, True);
  end;
end;
