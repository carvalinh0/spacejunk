# Installs spacejunk on Windows (PowerShell 5.1+).
#
#   irm https://raw.githubusercontent.com/carvalinh0/spacejunk/main/scripts/install.ps1 | iex
#
# 1. With cargo available: `cargo install spacejunk` (built for this machine).
# 2. Without cargo, or if that build fails (e.g. no Visual Studio Build Tools):
#    downloads the latest GitHub release, checks its SHA-256 and installs it to
#    %LOCALAPPDATA%\Programs\spacejunk, adding that folder to the user PATH.
# 3. Adds a Start Menu shortcut that opens spacejunk in a console window.
#
# Running it again updates to the latest version. Errors use `throw`, never
# `exit`: under `irm | iex` an exit would close the user's terminal.

function Install-Spacejunk {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue' # the progress bar makes downloads ~10x slower
    $repo = 'carvalinh0/spacejunk'
    $binDir = Join-Path $env:LOCALAPPDATA 'Programs\spacejunk'

    # Windows PowerShell 5.1 may default to TLS 1.0, which GitHub refuses.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    # Best effort: spacejunk is already installed when this runs.
    function New-StartMenuShortcut([string]$exe) {
        try {
            New-Item -ItemType Directory -Path $binDir -Force | Out-Null
            $icon = Join-Path $binDir 'spacejunk.ico'
            try {
                Invoke-WebRequest "https://raw.githubusercontent.com/$repo/main/assets/spacejunk.ico" -OutFile $icon -UseBasicParsing
            } catch {
                $icon = $null
            }
            $lnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\spacejunk.lnk'
            $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
            $shortcut.TargetPath = $exe
            $shortcut.WorkingDirectory = $env:USERPROFILE
            $shortcut.Description = 'Find out what is eating your disk'
            if ($icon) { $shortcut.IconLocation = "$icon,0" }
            $shortcut.Save()
            Write-Host 'spacejunk: added spacejunk to the Start Menu'
        } catch {
            Write-Host "spacejunk: could not create the Start Menu shortcut ($_); run spacejunk from a terminal"
        }
    }

    if (Get-Command cargo -ErrorAction SilentlyContinue) {
        Write-Host 'spacejunk: installing with cargo (cargo install spacejunk)...'
        & cargo install spacejunk --locked | Out-Host
        if ($LASTEXITCODE -eq 0) {
            $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
            New-StartMenuShortcut (Join-Path $cargoHome 'bin\spacejunk.exe')
            Write-Host 'spacejunk: done. Run: spacejunk (or open it from the Start Menu)'
            return
        }
        Write-Host 'spacejunk: cargo install failed; falling back to the prebuilt binary'
    }

    $arch = try { [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch { $env:PROCESSOR_ARCHITECTURE }
    $target = switch ($arch) {
        { $_ -in 'X64', 'AMD64' } { 'x86_64-pc-windows-msvc' }
        { $_ -in 'Arm64', 'ARM64' } { 'aarch64-pc-windows-msvc' }
        default { throw "spacejunk: unsupported CPU architecture $arch" }
    }

    try {
        $tag = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
    } catch {
        throw "spacejunk: could not reach the GitHub releases of $repo ($_)"
    }
    if (-not $tag) { throw "spacejunk: no release found for $repo" }

    $asset = "spacejunk-$tag-$target.zip"
    $url = "https://github.com/$repo/releases/download/$tag/$asset"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("spacejunk-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host "spacejunk: downloading $asset..."
        $zip = Join-Path $tmp $asset
        try {
            Invoke-WebRequest $url -OutFile $zip -UseBasicParsing
            Invoke-WebRequest "$url.sha256" -OutFile "$zip.sha256" -UseBasicParsing
        } catch {
            throw "spacejunk: no binary for $target in release $tag ($_)"
        }

        $expected = ((Get-Content "$zip.sha256" -Raw).Trim() -split '\s+')[0]
        $actual = (Get-FileHash $zip -Algorithm SHA256).Hash
        if ($actual -ne $expected) { throw "spacejunk: checksum mismatch for $asset" }

        Expand-Archive -Path $zip -DestinationPath $tmp -Force
        New-Item -ItemType Directory -Path $binDir -Force | Out-Null
        try {
            Copy-Item (Join-Path $tmp 'spacejunk.exe') (Join-Path $binDir 'spacejunk.exe') -Force
        } catch {
            throw "spacejunk: could not replace $binDir\spacejunk.exe; close spacejunk and run the installer again"
        }
    } finally {
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    Write-Host "spacejunk: installed $tag to $binDir\spacejunk.exe"

    if (($env:Path -split ';') -notcontains $binDir) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $newPath = if ($userPath) { "$userPath;$binDir" } else { $binDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = "$env:Path;$binDir"
        Write-Host "spacejunk: added $binDir to your user PATH (new terminals pick it up)"
    }
    New-StartMenuShortcut (Join-Path $binDir 'spacejunk.exe')
    Write-Host 'spacejunk: done. Run: spacejunk (or open it from the Start Menu)'
}

Install-Spacejunk
