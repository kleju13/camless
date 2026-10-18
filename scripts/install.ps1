# Installs or updates camless from the latest release: downloads camless-setup.exe, checks its
# SHA-256 and runs it silently. No admin needed; the app asks for it when it starts the service.
#   irm https://raw.githubusercontent.com/kleju13/camless/main/scripts/install.ps1 | iex
$ErrorActionPreference = 'Stop'
# Windows PowerShell downloads many times slower while it draws a progress bar.
$ProgressPreference = 'SilentlyContinue'
$github = 'kleju13/camless'

$release = Invoke-RestMethod "https://api.github.com/repos/$github/releases/latest" -Headers @{ 'User-Agent' = 'camless-install' } -UseBasicParsing
$asset = $release.assets | Where-Object { $_.name -eq 'camless-setup.exe' } | Select-Object -First 1
$sum = $release.assets | Where-Object { $_.name -eq 'camless-setup.exe.sha256' } | Select-Object -First 1
if (-not $asset -or -not $sum) { throw "release $($release.tag_name) has no camless-setup.exe" }

Write-Host "Downloading camless $($release.tag_name)..."
$dir = Join-Path $env:TEMP "camless-$($release.tag_name)"
New-Item -ItemType Directory -Force $dir | Out-Null
$setup = Join-Path $dir 'camless-setup.exe'
Invoke-WebRequest $asset.browser_download_url -OutFile $setup -UseBasicParsing
# Saved to a file: .Content of an octet-stream download is bytes, not text.
Invoke-WebRequest $sum.browser_download_url -OutFile "$setup.sha256" -UseBasicParsing
$want = ((Get-Content "$setup.sha256" -Raw) -split '\s+')[0]
$have = (Get-FileHash $setup -Algorithm SHA256).Hash
if ($have -ne $want) { Remove-Item $setup; throw "download does not match its checksum ($have, expected $want)" }
Unblock-File $setup

Write-Host 'Installing...'
# Not -Wait: that also waits for the app, which setup opens at the end and which keeps running.
$p = Start-Process $setup '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -PassThru
$null = $p.Handle  # without it, Windows PowerShell loses the exit code
$p.WaitForExit()
if ($p.ExitCode -ne 0) { throw "setup failed with exit code $($p.ExitCode)" }
Remove-Item $dir -Recurse -ErrorAction SilentlyContinue

Write-Host "Installed camless $($release.tag_name) in $env:LOCALAPPDATA\Programs\camless, with a Start menu entry."
Write-Host 'The app is opening: answer Yes when Windows asks for administrator rights, so CPU temperature works.'
