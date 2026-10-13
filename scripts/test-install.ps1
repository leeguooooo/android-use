param([string]$Archive)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$Archive = (Resolve-Path $Archive).Path
$temp = Join-Path ([IO.Path]::GetTempPath()) ('android-use-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $temp | Out-Null
$script:fixtureArchive = $Archive
$script:badChecksum = $false
function Invoke-WebRequest {
    param($Uri, $OutFile)
    if ($Uri.EndsWith('.sha256')) {
        $hash = if ($script:badChecksum) { '0' * 64 } else { (Get-FileHash $script:fixtureArchive -Algorithm SHA256).Hash }
        [IO.File]::WriteAllText($OutFile, "$hash  fixture.tar.gz`n")
    } elseif ($Uri.EndsWith('.tar.gz')) { Copy-Item $script:fixtureArchive $OutFile }
    else { throw 'Unexpected installer URL' }
}
try {
    & "$repo/install.ps1" -Tag v0.1.0 -InstallDir $temp -SkipPathUpdate
    $binary = Join-Path $temp 'android-use.exe'
    if (-not (Test-Path $binary)) { throw 'Windows installer did not install binary' }
    $before = (Get-FileHash $binary -Algorithm SHA256).Hash
    $script:badChecksum = $true
    $refused = $false
    try { & "$repo/install.ps1" -Tag v0.1.0 -InstallDir $temp -SkipPathUpdate } catch { $refused = $_.Exception.Message -match 'checksum' }
    if (-not $refused) { throw 'Windows installer accepted bad checksum' }
    if ((Get-FileHash $binary -Algorithm SHA256).Hash -ne $before) { throw 'Failed install replaced original binary' }
    Write-Output 'Windows installer: valid archive installs; bad checksum preserves original.'
} finally { Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue }
