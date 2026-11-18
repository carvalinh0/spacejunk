# Builds the Microsoft Store upload: one MSIX per architecture, bundled into a
# .msixbundle. Nothing is signed: the Store re-signs the package with a
# Microsoft certificate after certification (which is also what gets past
# Smart App Control). An unsigned bundle cannot be installed by hand.
#
#   ./packaging/msix/pack.ps1 -Exe @{ x64 = 'target/release/spacejunk.exe' } -Out spacejunk.msixbundle
#
# Needs makeappx.exe from the Windows SDK (installed on GitHub's Windows runners).
param(
    # MSIX architecture (x64, arm64) -> path of the spacejunk.exe built for it.
    [Parameter(Mandatory)] [hashtable] $Exe,
    [Parameter(Mandatory)] [string] $Out
)
$ErrorActionPreference = 'Stop'

$cargo = Get-Content (Join-Path $PSScriptRoot '../../Cargo.toml') -Raw
$crate = [regex]::Match($cargo, '(?m)^version = "(\d+\.\d+\.\d+)"').Groups[1].Value
if (-not $crate) { throw 'version not found in Cargo.toml' }
# The Store owns the fourth part and requires it to be 0 at upload. It also
# refuses a first part of 0, which is why spacejunk went from 0.1.1 to 1.0.0.
$version = "$crate.0"

$makeappx = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\10.*\x64\makeappx.exe' |
    Sort-Object { [version]$_.Directory.Parent.Name } |
    Select-Object -Last 1
if (-not $makeappx) { throw 'makeappx.exe not found: install the Windows SDK' }

$work = Join-Path ([IO.Path]::GetTempPath()) 'spacejunk-msix'
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
$packages = New-Item -ItemType Directory (Join-Path $work 'packages')
$manifest = Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw

foreach ($arch in $Exe.Keys) {
    $layout = New-Item -ItemType Directory (Join-Path $work $arch)
    Copy-Item $Exe[$arch] (Join-Path $layout 'spacejunk.exe')
    Copy-Item (Join-Path $PSScriptRoot 'Images') $layout -Recurse
    $manifest.Replace('{VERSION}', $version).Replace('{ARCH}', $arch) |
        Set-Content (Join-Path $layout 'AppxManifest.xml') -Encoding utf8
    & $makeappx pack /o /h SHA256 /d $layout /p (Join-Path $packages "spacejunk_$arch.msix")
    if ($LASTEXITCODE) { throw "makeappx pack failed for $arch" }
}

& $makeappx bundle /o /bv $version /d $packages /p $Out
if ($LASTEXITCODE) { throw 'makeappx bundle failed' }
Write-Host "Created $Out (version $version)"
