# tools/build.ps1
# Compiles the Rust host and packages the browser extensions into extension/dist/

param(
    [switch]$Release
)

$ErrorActionPreference = "Stop"

$src = Join-Path $PSScriptRoot "..\extension\src"
$distDir = Join-Path $PSScriptRoot "..\extension\dist"
$chromiumDst = Join-Path $distDir "chromium"
$firefoxDst = Join-Path $distDir "firefox"

Write-Host "Creating extension dist folders..." -ForegroundColor Cyan

# Clean and recreate dist folders
if (Test-Path $distDir) {
    Remove-Item $distDir -Recurse -Force
}
New-Item -ItemType Directory -Path $chromiumDst -Force | Out-Null
New-Item -ItemType Directory -Path $firefoxDst -Force | Out-Null

# 1. Copy shared files
Write-Host "  Copying shared extension files..." -ForegroundColor Yellow
Copy-Item "$src\*" -Destination $chromiumDst -Recurse -Force
Copy-Item "$src\*" -Destination $firefoxDst -Recurse -Force

# 2. Copy browser-specific manifests
Write-Host "  Copying manifest files..." -ForegroundColor Yellow
Copy-Item (Join-Path $PSScriptRoot "..\extension\chromium\manifest.json") -Destination (Join-Path $chromiumDst "manifest.json") -Force
Copy-Item (Join-Path $PSScriptRoot "..\extension\firefox\manifest.json") -Destination (Join-Path $firefoxDst "manifest.json") -Force

# 3. Read version from Rust Cargo.toml
$cargoTomlPath = Join-Path $PSScriptRoot "..\crates\host\Cargo.toml"
if (Test-Path $cargoTomlPath) {
    $toml = Get-Content $cargoTomlPath -Raw
    if ($toml -match '(?m)^version\s*=\s*"(.*?)"') {
        $version = $Matches[1]
        Write-Host "  Syncing version $version from Cargo.toml to manifests..." -ForegroundColor Green
    } else {
        $version = "0.1.0"
        Write-Host "  Warning: Could not find version in Cargo.toml. Defaulting to $version" -ForegroundColor Yellow
    }
} else {
    $version = "0.1.0"
}

# Helper to update manifest JSON using .NET to avoid formatting issues
function Update-ManifestJson {
    param(
        [string]$Path,
        [string]$Ver,
        [bool]$StripKey
    )
    $json = Get-Content $Path -Raw | ConvertFrom-Json
    $json.version = $Ver
    if ($StripKey -and $json.PSObject.Properties['key']) {
        $json.PSObject.Properties.Remove('key')
        Write-Host "  [Release Mode] Stripped developer 'key' from $Path" -ForegroundColor Yellow
    }
    # Convert to JSON with high depth to prevent truncation
    $updatedJson = ConvertTo-Json -InputObject $json -Depth 10
    # Write as UTF8
    [System.IO.File]::WriteAllText($Path, $updatedJson, [System.Text.Encoding]::UTF8)
}

Update-ManifestJson -Path (Join-Path $chromiumDst "manifest.json") -Ver $version -StripKey $Release
Update-ManifestJson -Path (Join-Path $firefoxDst "manifest.json") -Ver $version -StripKey $false

# 4. Compile Rust host binary
Write-Host "Compiling Rust host..." -ForegroundColor Cyan
Push-Location (Join-Path $PSScriptRoot "..")
if ($Release) {
    cargo build --release --manifest-path crates/host/Cargo.toml
} else {
    cargo build --manifest-path crates/host/Cargo.toml
}
Pop-Location

Write-Host "Extension and host builds completed successfully!" -ForegroundColor Green
