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
$DllDest     = Join-Path $InstallDir "turbrbtn.dll"
$ManifestPath = Join-Path $InstallDir "com.tur.native_host.json"

# Build host if missing
if (!(Test-Path $ReleaseHost) -and !(Test-Path $DebugHost)) {
    Write-Host "Building host..."
    Push-Location $ScriptDir
    cargo build --release --manifest-path crates/host/Cargo.toml
    Pop-Location
}

# Build button DLL if missing
if (!(Test-Path $ReleaseDll) -and !(Test-Path $DebugDll)) {
    Write-Host "Building button DLL..."
    Push-Location $ScriptDir
    cargo build --release --manifest-path crates/button/Cargo.toml
    Pop-Location
}

# Pick which binaries to install (prefer release)
$FinalExe = if (Test-Path $ReleaseHost) { $ReleaseHost } else { $DebugHost }
$FinalDll = if (Test-Path $ReleaseDll)  { $ReleaseDll  } else { $DebugDll }

if (!(Test-Path $FinalExe)) { throw "tur-native-host.exe not found after build" }
if (!(Test-Path $FinalDll))  { throw "turbrbtn.dll not found after build" }

Write-Host "Installing to $InstallDir"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $FinalExe -Destination $ExeDest -Force
Copy-Item -Path $FinalDll  -Destination $DllDest  -Force
Write-Host "  binaries installed"

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

Write-Host ""
Write-Host "Installation complete!"
Write-Host "Extension ID: $ExtensionId"
