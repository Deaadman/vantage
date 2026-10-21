# Points vrfkit.json at vrfkit's newest release, or -Tag, after checking it:
#
#   - the ZIP's SHA-256 matches GitHub's own record of the file
#   - vrfkit.exe inside matches the build-info.json shipped next to it
#   - the VALORANT builds it decodes come from its README at that tag
#
# The line players see is "Supports VALORANT <build>" when the release reads
# a newer build than before, and empty otherwise; edit "notes" in vrfkit.json
# to say something else. The vrfkit-watch workflow runs this once a day and
# opens a pull request; merging it sends the new vrfkit to every install.
#
#   .\scripts\vrfkit-bump.ps1                  # the newest release
#   .\scripts\vrfkit-bump.ps1 -Tag v0.4.0
#   .\scripts\vrfkit-bump.ps1 -OutDir bump     # also write bump/pr-body.md
#
# Does nothing when vrfkit.json already names that release or a newer one.
param(
    [string]$Tag,
    [string]$OutDir,
    [string]$PinFile = (Join-Path $PSScriptRoot '..\vrfkit.json')
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'yakisoba0728/vrfkit'
$Api = "https://api.github.com/repos/$Repo"
$headers = @{ 'User-Agent' = 'vantage-vrfkit-bump'; Accept = 'application/vnd.github+json' }
$token = if ($env:GH_TOKEN) { $env:GH_TOKEN } else { $env:GITHUB_TOKEN }
if ($token) { $headers.Authorization = "Bearer $token" }

# v0.4.0 or 13.08 as a [version] for comparing.
function ConvertTo-Version([string]$v) { [version](($v.TrimStart('v') -split '[-+]')[0]) }

function Set-Output([string]$Name, [string]$Value) {
    if ($env:GITHUB_OUTPUT) { "$Name=$Value" >> $env:GITHUB_OUTPUT }
}

$pin = Get-Content -Raw $PinFile | ConvertFrom-Json
$release = if ($Tag) { Invoke-RestMethod "$Api/releases/tags/$Tag" -Headers $headers } else { Invoke-RestMethod "$Api/releases/latest" -Headers $headers }
$tag = $release.tag_name
if ((ConvertTo-Version $tag) -le (ConvertTo-Version $pin.tag)) {
    Write-Host "vrfkit.json names $($pin.tag); vrfkit's release $tag is no newer"
    exit 0
}

# Download the Windows build and check it.
$assetName = "vrfkit-$tag-windows-x64.zip"
$asset = $release.assets | Where-Object name -eq $assetName | Select-Object -First 1
if (-not $asset) { throw "vrfkit $tag has no $assetName" }
$work = Join-Path ([IO.Path]::GetTempPath()) "vrfkit-bump-$tag-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$zip = Join-Path $work $assetName
Invoke-WebRequest $asset.browser_download_url -OutFile $zip -Headers @{ 'User-Agent' = 'vantage-vrfkit-bump' }
$zipHash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLowerInvariant()
if (-not $asset.digest) { throw "GitHub has no SHA-256 on record for $assetName, so it can't be checked" }
if ($asset.digest -ne "sha256:$zipHash") { throw "$assetName has SHA-256 $zipHash, but GitHub's record says $($asset.digest)" }

Expand-Archive $zip (Join-Path $work 'x') -Force
$exe = Get-ChildItem (Join-Path $work 'x') -Recurse -Filter vrfkit.exe | Select-Object -First 1
if (-not $exe) { throw "$assetName has no vrfkit.exe" }
$exeHash = (Get-FileHash -Algorithm SHA256 $exe.FullName).Hash.ToLowerInvariant()
$info = Get-ChildItem (Join-Path $work 'x') -Recurse -Filter build-info.json | Select-Object -First 1
if ($info) {
    $built = Get-Content -Raw $info.FullName | ConvertFrom-Json
    if ($built.executable_sha256 -ne $exeHash) { throw "vrfkit.exe has SHA-256 $exeHash, but its build-info.json says $($built.executable_sha256)" }
    if ($built.tag -ne $tag) { throw "build-info.json says $($built.tag), not $tag" }
}

# The builds it decodes: README, "Supported VALORANT builds", rows like | **13.08** |.
$readme = Invoke-RestMethod "$Api/contents/README.md?ref=$tag" -Headers $headers
$text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($readme.content))
$section = [regex]::Match($text, '(?ms)^## Supported VALORANT builds\s*$(.*?)(?=^## )')
if (-not $section.Success) { throw "vrfkit's README at $tag has no 'Supported VALORANT builds' section" }
$builds = [regex]::Matches($section.Groups[1].Value, '\|\s*\*\*(\d+\.\d+)\*\*\s*\|') |
    ForEach-Object { $_.Groups[1].Value } | Sort-Object { ConvertTo-Version $_ }
if (-not $builds) { throw "found no builds in vrfkit's README at $tag" }
$oldest = @($builds)[0]
$newest = @($builds)[-1]
# @() keeps one line a list: PowerShell unrolls what an `if` returns.
$notes = @(if ((ConvertTo-Version $newest) -gt (ConvertTo-Version $pin.builds[1])) { "Supports VALORANT $newest" })

# Same layout as the committed file, so the diff shows only what changed.
$noteList = ($notes | ForEach-Object { '"' + ($_ -replace '\\', '\\' -replace '"', '\"') + '"' }) -join ', '
$json = @"
{
  "tag": "$tag",
  "zip_url": "$($asset.browser_download_url)",
  "zip_sha256": "$zipHash",
  "exe_sha256": "$exeHash",
  "builds": ["$oldest", "$newest"],
  "notes": [$noteList],
  "min_vantage": "$($pin.min_vantage)"
}

"@
[IO.File]::WriteAllText((Resolve-Path $PinFile), $json)
Remove-Item -Recurse -Force $work
Write-Host "vrfkit.json now names vrfkit $tag (VALORANT $oldest to $newest)"
Set-Output 'tag' $tag

if ($OutDir) {
    New-Item -ItemType Directory -Force $OutDir | Out-Null
    $line = if ($notes) { "> $($notes[0])" } else { "Nothing: this release reads no newer VALORANT build, so players see only ""vrfkit updated to $tag"" and its link." }
    $published = ([datetime]$release.published_at).ToString('MMM d, yyyy', [Globalization.CultureInfo]::InvariantCulture)
    $body = @"
[vrfkit $tag]($($release.html_url)) was released on $published. This points ``vrfkit.json`` at it. Merging sends it to every install on its next update check, with no Vantage release.

### What players will see

$line

To say something else, edit ``notes`` in ``vrfkit.json`` on this branch before merging. If this vrfkit changes what Vantage reads, raise ``min_vantage`` to the first Vantage that handles it.

### Checked

- The ZIP's SHA-256 matches GitHub's record of the file, and ``vrfkit.exe`` matches its ``build-info.json``.
- Decodes VALORANT $oldest to $newest, from vrfkit's README.
- CI runs every test with this vrfkit, including reading the public replays.

<details><summary>vrfkit's notes for $tag</summary>

$($release.body)

</details>
"@
    [IO.File]::WriteAllText((Join-Path $OutDir 'pr-body.md'), $body)
}
