# Installs valreview.
#
#   irm https://raw.githubusercontent.com/Deaadman/valreview/main/install.ps1 | iex
#
# Downloads the release's valreview.exe, checks it against the release
# manifest, and runs `valreview install`, which copies it to
# %LOCALAPPDATA%\valreview and fetches the vrfkit release it pins. The bin
# folder is added to your user PATH. After that valreview updates itself.
#
# Set VALREVIEW_VERSION to a release tag (for example v0.1.0) to install that
# release instead of the latest.

param(
    [string]$Version = $env:VALREVIEW_VERSION,
    # A local valreview.exe to install instead of downloading (for testing).
    [string]$Exe = $env:VALREVIEW_EXE
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'Deaadman/valreview'
$Platform = 'windows-x64'

if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'valreview needs 64-bit Windows.'
}

$Root = if ($env:VALREVIEW_HOME) { $env:VALREVIEW_HOME } else { Join-Path $env:LOCALAPPDATA 'valreview' }
$Bin = Join-Path $Root 'bin'
$Temp = Join-Path ([IO.Path]::GetTempPath()) ("valreview-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $Temp | Out-Null

try {
    Write-Host ''
    Write-Host 'Installing valreview'

    $download = Join-Path $Temp 'valreview.exe'
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
        Write-Host "  Downloading valreview $($manifest.version)"
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

    # valreview copies itself into place and fetches vrfkit.
    & $download install
    if ($LASTEXITCODE -ne 0) { throw 'valreview install failed' }

    Write-Host ''
    Write-Host 'Run `valreview` to start. Open a new terminal first if it is not found.'
    Write-Host ''
} finally {
    Remove-Item -LiteralPath $Temp -Recurse -Force -ErrorAction SilentlyContinue
}
