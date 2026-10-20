# Installs valreview and the vrfkit release it is pinned to.
#
#   irm https://raw.githubusercontent.com/Deaadman/valreview/main/install.ps1 | iex
#
# valreview.exe and vrfkit.exe go to %LOCALAPPDATA%\valreview\bin, which is
# added to your user PATH. Run it again to update. Set VALREVIEW_VERSION to a
# release tag (for example v0.1.0) to install that release instead of the latest.

param(
    [string]$Version = $env:VALREVIEW_VERSION,
    [string]$Archive = $env:VALREVIEW_ARCHIVE
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'Deaadman/valreview'
$Asset = 'valreview-windows-x64.zip'

# vrfkit release valreview checks against (crates/core/src/vrfkit.rs).
$VrfkitTag = 'v0.3.0'
$VrfkitZip = "vrfkit-$VrfkitTag-windows-x64.zip"
$VrfkitZipSha256 = '58cd76fb926642de931e74d70b3ad3db1e696ffbd752e4a1a60fc6686c4540df'
$VrfkitExeSha256 = '1a211eebbaf0a794201baefa2b32162d74c93fec38a8db3a793c0d93429ba0ca'

function Write-Step($Message) { Write-Host "  $Message" }

function Get-Sha256($Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }

function Assert-Sha256($Path, $Expected, $What) {
    $actual = Get-Sha256 $Path
    if ($actual -ne $Expected.ToLowerInvariant()) {
        throw "$What has SHA-256 $actual, expected $Expected"
    }
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'valreview needs 64-bit Windows.'
}

$Root = Join-Path $env:LOCALAPPDATA 'valreview'
$Bin = Join-Path $Root 'bin'
$Temp = Join-Path ([IO.Path]::GetTempPath()) ("valreview-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $Bin, $Temp | Out-Null

try {
    Write-Host ''
    Write-Host 'Installing valreview'

    # valreview
    $zip = Join-Path $Temp $Asset
    if ($Archive) {
        Write-Step "Using $Archive"
        Copy-Item -LiteralPath $Archive -Destination $zip
    } else {
        if ($Version) {
            $base = "https://github.com/$Repo/releases/download/$Version"
        } else {
            $base = "https://github.com/$Repo/releases/latest/download"
        }
        Write-Step "Downloading $Asset$(if ($Version) { " ($Version)" })"
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$Asset" -OutFile $zip
        $sum = (Invoke-WebRequest -UseBasicParsing -Uri "$base/$Asset.sha256").Content
        if ($sum -is [byte[]]) { $sum = [Text.Encoding]::ASCII.GetString($sum) }
        Assert-Sha256 $zip ($sum.Trim() -split '\s+')[0] $Asset
    }
    $out = Join-Path $Temp 'valreview'
    Expand-Archive -LiteralPath $zip -DestinationPath $out
    $exe = Get-ChildItem -LiteralPath $out -Recurse -Filter 'valreview.exe' | Select-Object -First 1
    if (-not $exe) { throw "$Asset has no valreview.exe" }
    Copy-Item -LiteralPath $exe.FullName -Destination (Join-Path $Bin 'valreview.exe') -Force

    # vrfkit
    $vrfkit = Join-Path $Bin 'vrfkit.exe'
    if ((Test-Path -LiteralPath $vrfkit) -and (Get-Sha256 $vrfkit) -eq $VrfkitExeSha256) {
        Write-Step "vrfkit $VrfkitTag already installed"
    } else {
        Write-Step "Downloading vrfkit $VrfkitTag"
        $vzip = Join-Path $Temp $VrfkitZip
        Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/yakisoba0728/vrfkit/releases/download/$VrfkitTag/$VrfkitZip" -OutFile $vzip
        Assert-Sha256 $vzip $VrfkitZipSha256 $VrfkitZip
        $vout = Join-Path $Temp 'vrfkit'
        Expand-Archive -LiteralPath $vzip -DestinationPath $vout
        $vexe = Get-ChildItem -LiteralPath $vout -Recurse -Filter 'vrfkit.exe' | Select-Object -First 1
        if (-not $vexe) { throw "$VrfkitZip has no vrfkit.exe" }
        Assert-Sha256 $vexe.FullName $VrfkitExeSha256 'vrfkit.exe'
        Copy-Item -LiteralPath $vexe.FullName -Destination $vrfkit -Force
    }

    # PATH
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries -notcontains $Bin) {
        [Environment]::SetEnvironmentVariable('Path', (($entries + $Bin) -join ';'), 'User')
        Write-Step "Added $Bin to your PATH"
    }
    if (($env:Path -split ';') -notcontains $Bin) { $env:Path = "$env:Path;$Bin" }

    # Point valreview at vrfkit; keeps any other settings in config.toml.
    & (Join-Path $Bin 'valreview.exe') init --vrfkit $vrfkit
    if ($LASTEXITCODE -ne 0) { throw 'valreview init failed' }

    $installed = (& (Join-Path $Bin 'valreview.exe') --version) -join ' '
    Write-Host ''
    Write-Host "$installed installed to $Bin"
    Write-Host 'Run `valreview` to start. Open a new terminal first if it is not found.'
    Write-Host ''
} finally {
    Remove-Item -LiteralPath $Temp -Recurse -Force -ErrorAction SilentlyContinue
}
