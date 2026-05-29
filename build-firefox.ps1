# build-firefox.ps1
# Regenerates extension-firefox/ from extension/.
# Run this any time you change extension JS/CSS/icons.
$src = Join-Path $PSScriptRoot "extension"
$dst = Join-Path $PSScriptRoot "extension-firefox"
if (Test-Path $dst) { Remove-Item $dst -Recurse -Force }
Copy-Item $src $dst -Recurse
Copy-Item "$dst\manifest.firefox.json" "$dst\manifest.json" -Force
Remove-Item "$dst\manifest.firefox.json" -Force
Write-Host "extension-firefox/ rebuilt OK"
