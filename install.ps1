# Installs Vantage.
#
#   irm https://raw.githubusercontent.com/Deaadman/vantage/main/install.ps1 | iex
#
# Downloads the release's vantage.exe, checks it against the release
# manifest, and runs `vantage install`, which copies it to
# %LOCALAPPDATA%\vantage and fetches the vrfkit release it pins. ffmpeg,
# which takes the screenshots, comes from winget if it isn't installed. The
# bin folder is added to your user PATH. After that Vantage updates itself.
#
# Set VANTAGE_VERSION to a release tag (for example v0.1.0) to install that
# release instead of the latest.

param(
    [string]$Version = $env:VANTAGE_VERSION,
    # A local vantage.exe to install instead of downloading (for testing).
    [string]$Exe = $env:VANTAGE_EXE
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'Deaadman/vantage'
$Platform = 'windows-x64'

if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'Vantage needs 64-bit Windows.'
}

$Root = if ($env:VANTAGE_HOME) { $env:VANTAGE_HOME } else { Join-Path $env:LOCALAPPDATA 'vantage' }
$Bin = Join-Path $Root 'bin'
$Temp = Join-Path ([IO.Path]::GetTempPath()) ("vantage-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $Temp | Out-Null

try {
    Write-Host ''
    Write-Host 'Installing Vantage'

    $download = Join-Path $Temp 'vantage.exe'
    if ($Exe) {
        Write-Host "  Using $Exe"
        Copy-Item -LiteralPath $Exe -Destination $download
    } else {
        if ($Version) {
            $manifestUrl = "https://github.com/$Repo/releases/download/$Version/manifest.json"
        } else {
            $manifestUrl = "https://github.com/$Repo/releases/latest/download/manifest.json"
        }
        $manifest = Invoke-RestMethod -UseBasicParsing -Uri $manifestUrl
        $asset = $manifest.platforms.$Platform
        if (-not $asset) { throw "release $($manifest.version) has no $Platform build" }
        Write-Host "  Downloading Vantage $($manifest.version)"
        Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/$Repo/releases/download/v$($manifest.version)/$($asset.file)" -OutFile $download
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $download).Hash.ToLowerInvariant()
        if ($actual -ne $asset.sha256.ToLowerInvariant()) {
            throw "$($asset.file) has SHA-256 $actual, expected $($asset.sha256)"
        }
    }

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries -notcontains $Bin) {
        [Environment]::SetEnvironmentVariable('Path', (($entries + $Bin) -join ';'), 'User')
        Write-Host "  Added $Bin to your PATH"
    }
    if (($env:Path -split ';') -notcontains $Bin) { $env:Path = "$env:Path;$Bin" }

    # Vantage copies itself into place and fetches vrfkit.
    & $download install
    if ($LASTEXITCODE -ne 0) { throw 'vantage install failed' }

    # ffmpeg takes the screenshots of your deaths from your clips. Without it
    # Vantage still works, and its status bar says /update installs it.
    $wingetLinks = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Links\ffmpeg.exe'
    if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue) -and -not (Test-Path $wingetLinks)) {
        if (Get-Command winget -ErrorAction SilentlyContinue) {
            Write-Host '  Installing ffmpeg for screenshots (winget)'
            winget install --id Gyan.FFmpeg --exact --silent --accept-source-agreements --accept-package-agreements | Out-Null
            if ($LASTEXITCODE -ne 0) { Write-Host '  ffmpeg could not be installed; /update tries again' }
        } else {
            Write-Host '  winget was not found, so ffmpeg was not installed; /update tries again'
        }
    }

    Write-Host ''
    Write-Host 'Run `vantage` to start. Open a new terminal first if it is not found.'
    Write-Host ''
} finally {
    Remove-Item -LiteralPath $Temp -Recurse -Force -ErrorAction SilentlyContinue
}
