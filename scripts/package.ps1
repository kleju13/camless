# Builds camless.exe and camless-gui.exe and packs them into dist\camless-setup.exe with Inno Setup,
# next to its SHA-256 in dist\camless-setup.exe.sha256. CI runs it for every pull request, and
# release.yml for every tag; -Tag v0.2.0 fails unless every crate is at version 0.2.0.
param([string]$Tag)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

# The app compares its own version with the latest release, so all three crates move together.
$versions = 'Cargo.toml', 'core\Cargo.toml', 'gui\Cargo.toml' | ForEach-Object {
    (Select-String -Path (Join-Path $root $_) -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
}
$version = $versions[0]
if (@($versions | Select-Object -Unique).Count -ne 1) { throw "the crates are at different versions: $($versions -join ', ')" }
if ($Tag -and $Tag -ne "v$version") { throw "tag $Tag does not match version $version in Cargo.toml" }

cargo build --release --workspace --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = Get-ChildItem "${env:ProgramFiles(x86)}\Inno Setup *\ISCC.exe", "$env:ProgramFiles\Inno Setup *\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup *\ISCC.exe" -ErrorAction SilentlyContinue |
        Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $iscc) { throw 'Inno Setup is not installed: https://jrsoftware.org/isdl.php' }
# Windows file versions are numbers only, so 0.5.0-beta.1 is 0.5.0 there.
$numeric = ($version -split '[-+]')[0]
& $iscc /Q "/DAppVersion=$version" "/DAppNumericVersion=$numeric" (Join-Path $root 'installer\camless.iss')
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }

$setup = Join-Path $root 'dist\camless-setup.exe'
"$((Get-FileHash $setup -Algorithm SHA256).Hash.ToLower())  camless-setup.exe" | Out-File -Encoding ascii "$setup.sha256"
Write-Host "Built $setup (camless $version)"
