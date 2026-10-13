param([string]$Tag = $env:ANDROID_USE_TAG, [string]$InstallDir = "$env:USERPROFILE\.local\bin")
$ErrorActionPreference = 'Stop'
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { throw 'This release supports Windows x64.' }
if (-not $Tag) { $Tag = (Invoke-RestMethod 'https://api.github.com/repos/leeguooooo/android-use/releases/latest').tag_name }
if ($Tag -notmatch '^v[0-9][a-zA-Z0-9._-]*$') { throw 'Invalid release tag' }
$temp = Join-Path ([IO.Path]::GetTempPath()) ('android-use-install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    $asset = 'android-use-windows-x86_64.tar.gz'
    $base = "https://github.com/leeguooooo/android-use/releases/download/$Tag"
    $archive = Join-Path $temp $asset
    Invoke-WebRequest "$base/$asset" -OutFile $archive
    Invoke-WebRequest "$base/$asset.sha256" -OutFile "$archive.sha256"
    $expected = ((Get-Content "$archive.sha256" -Raw).Trim() -split '\s+')[0]
    if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLower() -ne $expected.ToLower()) { throw 'Archive checksum mismatch' }
    & tar -xzf $archive -C $temp
    if ($LASTEXITCODE -ne 0) { throw 'Archive extraction failed' }
    $binary = Join-Path $temp 'android-use.exe'
    & $binary --version
    if ($LASTEXITCODE -ne 0) { throw 'Downloaded binary cannot run' }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $target = Join-Path $InstallDir 'android-use.exe'
    $backup = "$target.previous"
    if (Test-Path $backup) { Remove-Item $backup -Force }
    if (Test-Path $target) { Move-Item $target $backup -Force }
    try { Copy-Item $binary $target -Force } catch { if (Test-Path $backup) { Move-Item $backup $target -Force }; throw }
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $InstallDir) { [Environment]::SetEnvironmentVariable('Path', "$InstallDir;$userPath", 'User') }
    Write-Output "Installed $target. Open a new terminal; run android-use doctor, then android-use connect."
} finally { Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue }
