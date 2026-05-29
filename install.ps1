# Tur Native Overlay Installer
# Installs the native messaging host and turbrbtn.dll

param(
    [string]$ExtensionId = "nmfbhanlffgjbdnnbckoadbpkonfpacl"
)

$ScriptDir = Split-Path -Parent $PSCommandPath
$ReleaseHost = Join-Path $ScriptDir "target\release\tur-native-host.exe"
$DebugHost   = Join-Path $ScriptDir "target\debug\tur-native-host.exe"
$ReleaseDll  = Join-Path $ScriptDir "target\release\turbrbtn.dll"
$DebugDll    = Join-Path $ScriptDir "target\debug\turbrbtn.dll"
$InstallDir  = Join-Path $env:LOCALAPPDATA "tur-native-host"
$ExeDest     = Join-Path $InstallDir "tur-native-host.exe"
$ManifestPath = Join-Path $InstallDir "com.tur.native_host.json"

# Build host if missing
if (!(Test-Path $ReleaseHost) -and !(Test-Path $DebugHost)) {
    Write-Host "Building host..."
    Push-Location $ScriptDir
    cargo build --release --manifest-path crates/host/Cargo.toml
    Pop-Location
}

# Pick which binary to install (prefer release)
$FinalExe = if (Test-Path $ReleaseHost) { $ReleaseHost } else { $DebugHost }

if (!(Test-Path $FinalExe)) { throw "tur-native-host.exe not found. Run 'cargo build -p tur-native-host' first." }

Write-Host "Installing to $InstallDir"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# Kill any running instance so the exe file is not locked during copy.
$running = Get-Process -Name "tur-native-host" -ErrorAction SilentlyContinue
if ($running) {
    Write-Host "  stopping running tur-native-host process(es)..."
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 600   # give OS time to release file handles
}

Copy-Item -Path $FinalExe -Destination $ExeDest -Force
Write-Host "  binary installed: $FinalExe -> $ExeDest"

Copy-Item -Path (Join-Path $ScriptDir "crates\host\src\InstrumentSans.ttf") -Destination (Join-Path $InstallDir "InstrumentSans.ttf") -Force
Copy-Item -Path (Join-Path $ScriptDir "crates\host\src\LeMurmure.otf") -Destination (Join-Path $InstallDir "LeMurmure.otf") -Force
Write-Host "  custom fonts installed to $InstallDir"

# Generate manifest with correct extension ID and full binary path
$Manifest = @{
    name            = "com.tur.native_host"
    description     = "tur Download Manager native messaging host"
    path            = $ExeDest
    type            = "stdio"
    allowed_origins = @("chrome-extension://$ExtensionId/")
}
$Manifest | ConvertTo-Json -Compress | Set-Content -Path $ManifestPath -Encoding UTF8
Write-Host "  manifest installed"

# Register in registry (HKCU = no admin required)
$RegistryPaths = @(
    "HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.tur.native_host",
    "HKCU:\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.tur.native_host",
    "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.tur.native_host",
    "HKCU:\Software\Chromium\NativeMessagingHosts\com.tur.native_host"
)

foreach ($RegPath in $RegistryPaths) {
    if (-not (Test-Path $RegPath)) {
        New-Item -Path $RegPath -Force | Out-Null
    }
    Set-ItemProperty -Path $RegPath -Name "(default)" -Value $ManifestPath -Force
    Write-Host "  registered: $RegPath"
}

# ── Firefox / Gecko manifest ──────────────────────────────────────────────────
# Firefox requires allowed_extensions (array of gecko IDs) instead of
# allowed_origins. The Rust binary is 100% compatible — same stdio protocol.
$FirefoxManifest = @{
    name               = "com.tur.native_host"
    description        = "tur Download Manager native messaging host"
    path               = $ExeDest
    type               = "stdio"
    allowed_extensions = @("tur@project.local")
}
$FirefoxManifestPath = Join-Path $InstallDir "com.tur.native_host.firefox.json"
$FirefoxManifest | ConvertTo-Json -Compress | Set-Content -Path $FirefoxManifestPath -Encoding UTF8
Write-Host "  Firefox manifest: $FirefoxManifestPath"

$FirefoxRegistryPaths = @(
    "HKCU:\Software\Mozilla\NativeMessagingHosts\com.tur.native_host",
    "HKCU:\Software\Mozilla\Firefox\NativeMessagingHosts\com.tur.native_host"
)

foreach ($RegPath in $FirefoxRegistryPaths) {
    if (-not (Test-Path $RegPath)) {
        New-Item -Path $RegPath -Force | Out-Null
    }
    Set-ItemProperty -Path $RegPath -Name "(default)" -Value $FirefoxManifestPath -Force
    Write-Host "  registered (Firefox): $RegPath"
}

Write-Host ""
Write-Host "Installation complete!"
Write-Host "Extension ID (Chromium): $ExtensionId"
Write-Host "Extension ID (Firefox):  tur@project.local"
