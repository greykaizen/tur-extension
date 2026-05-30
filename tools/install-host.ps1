# tools/install-host.ps1
# Installs and registers the native messaging host in the Windows Registry

param(
    [string]$ExtensionId = "nkilabbnigegcggilmdjaepemndfance", # Default locked developer ID
    [string]$GeckoId = "tur@project.local",
    [switch]$Release
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $PSCommandPath
$ProjectRoot = Split-Path -Parent $ScriptDir
$ReleaseHost = Join-Path $ProjectRoot "target\release\tur-native-host.exe"
$DebugHost   = Join-Path $ProjectRoot "target\debug\tur-native-host.exe"
$InstallDir  = Join-Path $env:LOCALAPPDATA "tur-native-host"
$ExeDest     = Join-Path $InstallDir "tur-native-host.exe"
$ManifestPath = Join-Path $InstallDir "com.tur.native_host.json"

# Pick which binary to install based on compile mode
$FinalExe = $null
if ($Release) {
    if (Test-Path $ReleaseHost) { $FinalExe = $ReleaseHost }
    elseif (Test-Path $DebugHost) { $FinalExe = $DebugHost }
} else {
    if (Test-Path $DebugHost) { $FinalExe = $DebugHost }
    elseif (Test-Path $ReleaseHost) { $FinalExe = $ReleaseHost }
}

if ($null -eq $FinalExe -or !(Test-Path $FinalExe)) {
    throw "tur-native-host.exe not found. Run '.\dev.ps1 build' first."
}

Write-Host "Installing Native Host to $InstallDir" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

# 1. Kill any running host processes to release file locks
$running = Get-Process -Name "tur-native-host" -ErrorAction SilentlyContinue
if ($running) {
    Write-Host "  Stopping running tur-native-host process(es)..." -ForegroundColor Yellow
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 600
}

# 2. Copy binary and custom fonts
Copy-Item -Path $FinalExe -Destination $ExeDest -Force
Write-Host "  Binary copied: $FinalExe -> $ExeDest" -ForegroundColor Green

$fontsSrc = Join-Path $ProjectRoot "crates\host\resources"
Copy-Item -Path (Join-Path $fontsSrc "InstrumentSans.ttf") -Destination (Join-Path $InstallDir "InstrumentSans.ttf") -Force
Copy-Item -Path (Join-Path $fontsSrc "LeMurmure.otf") -Destination (Join-Path $InstallDir "LeMurmure.otf") -Force
Write-Host "  Custom fonts installed." -ForegroundColor Green

# 3. Generate & Register Chromium Native Messaging Host Manifest
$origins = @(
    "chrome-extension://nkilabbnigegcggilmdjaepemndfance/", # Default developer ID
    "chrome-extension://omhacdegdipjjakobgakailcbgbhgbpd/"  # User's Brave ID
)
if ($origins -notcontains "chrome-extension://$ExtensionId/") {
    $origins += "chrome-extension://$ExtensionId/"
}

$Manifest = @{
    name            = "com.tur.native_host"
    description     = "tur Download Manager native messaging host"
    path            = $ExeDest
    type            = "stdio"
    allowed_origins = $origins
}
$Manifest | ConvertTo-Json -Compress | Set-Content -Path $ManifestPath -Encoding UTF8
Write-Host "  Chromium manifest generated (origins: $($origins -join ', '))" -ForegroundColor Green

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
    Write-Host "  Registered registry path: $RegPath" -ForegroundColor Green
}

# 4. Generate & Register Firefox/Gecko Native Messaging Host Manifest
$FirefoxManifest = @{
    name               = "com.tur.native_host"
    description        = "tur Download Manager native messaging host"
    path               = $ExeDest
    type               = "stdio"
    allowed_extensions = @($GeckoId)
}
$FirefoxManifestPath = Join-Path $InstallDir "com.tur.native_host.firefox.json"
$FirefoxManifest | ConvertTo-Json -Compress | Set-Content -Path $FirefoxManifestPath -Encoding UTF8
Write-Host "  Firefox manifest generated (ID: $GeckoId)" -ForegroundColor Green

$FirefoxRegistryPaths = @(
    "HKCU:\Software\Mozilla\NativeMessagingHosts\com.tur.native_host",
    "HKCU:\Software\Mozilla\Firefox\NativeMessagingHosts\com.tur.native_host"
)

foreach ($RegPath in $FirefoxRegistryPaths) {
    if (-not (Test-Path $RegPath)) {
        New-Item -Path $RegPath -Force | Out-Null
    }
    Set-ItemProperty -Path $RegPath -Name "(default)" -Value $FirefoxManifestPath -Force
    Write-Host "  Registered registry path (Firefox): $RegPath" -ForegroundColor Green
}

Write-Host "`nInstallation successfully completed!" -ForegroundColor Green
