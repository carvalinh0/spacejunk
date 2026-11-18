# Builds the Microsoft Store upload: one MSIX per architecture, bundled into a
# .msixbundle, plus the same bundle zipped as .msixupload (what Partner Center
# recommends and what `msstore publish` accepts; it takes no .msixbundle). Nothing is signed: the Store re-signs the package with a
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
$makepri = Join-Path $makeappx.Directory 'makepri.exe'

$work = Join-Path ([IO.Path]::GetTempPath()) 'spacejunk-msix'
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
$packages = New-Item -ItemType Directory (Join-Path $work 'packages')
$manifest = Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw
# Images/ holds each logo per scale (scale-100..400) and per taskbar size
# (targetsize-16..256). Windows picks the right one through resources.pri;
# without it, only an unqualified file can be used, blurry when scaled up.
$priconfig = Join-Path $work 'priconfig.xml'
& $makepri createconfig /cf $priconfig /dq en-US /pv 10.0.0 /o
if ($LASTEXITCODE) { throw 'makepri createconfig failed' }

foreach ($arch in $Exe.Keys) {
    $layout = New-Item -ItemType Directory (Join-Path $work $arch)
    Copy-Item $Exe[$arch] (Join-Path $layout 'spacejunk.exe')
    Copy-Item (Join-Path $PSScriptRoot 'Images') $layout -Recurse
    $manifest.Replace('{VERSION}', $version).Replace('{ARCH}', $arch) |
        Set-Content (Join-Path $layout 'AppxManifest.xml') -Encoding utf8
    & $makepri new /pr $layout /cf $priconfig /mn (Join-Path $layout 'AppxManifest.xml') /of (Join-Path $layout 'resources.pri') /o
    if ($LASTEXITCODE) { throw "makepri failed for $arch" }
    & $makeappx pack /o /h SHA256 /d $layout /p (Join-Path $packages "spacejunk_$arch.msix")
    if ($LASTEXITCODE) { throw "makeappx pack failed for $arch" }
}

& $makeappx bundle /o /bv $version /d $packages /p $Out
if ($LASTEXITCODE) { throw 'makeappx bundle failed' }
# Compress-Archive only writes .zip files.
$upload = [IO.Path]::ChangeExtension($Out, '.msixupload')
Compress-Archive -Path $Out -DestinationPath "$upload.zip" -Force
Move-Item -Force "$upload.zip" $upload
Write-Host "Created $Out and $upload (version $version)"
