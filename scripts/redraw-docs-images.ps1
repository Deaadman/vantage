# Redraws the pictures in docs/images from Vantage's own screens, on made-up
# matches. Run it after changing a screen the docs show, then look over the
# changed pictures before committing. Needs the network for the map catalog.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
cargo test -p vantage redraw_docs_images -- --ignored --nocapture
if ($LASTEXITCODE -ne 0) { throw 'redrawing the docs pictures failed' }
git status --short docs/images
