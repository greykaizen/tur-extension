# dev.ps1
# Main entry point for Project Tur developer tooling CLI

param(
    [Parameter(Position=0, Mandatory=$true)]
    [ValidateSet("build", "install", "clean", "run-chrome", "run-firefox")]
    [string]$Action,

    [switch]$Release,
    [string]$ExtensionId,
    [string]$GeckoId
)

$ErrorActionPreference = "Stop"
$ProjectRoot = $PSScriptRoot

# ── Helper to resolve browser paths ───────────────────────────────────────────

function Get-ChromePath {
    $paths = @(
        "C:\Program Files\Google\Chrome\Application\chrome.exe",
        "C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
    )
    foreach ($p in $paths) { if (Test-Path $p) { return $p } }
    $cmd = Get-Command chrome.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Get-FirefoxPath {
    $paths = @(
        "C:\Program Files\Mozilla Firefox\firefox.exe",
        "C:\Program Files (x86)\Mozilla Firefox\firefox.exe"
    )
    foreach ($p in $paths) { if (Test-Path $p) { return $p } }
    $cmd = Get-Command firefox.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

# ── Subcommand execution ──────────────────────────────────────────────────────

switch ($Action) {
    "build" {
        $buildArgs = @()
        if ($Release) { $buildArgs += "-Release" }
        
        Write-Host "Running build tool..." -ForegroundColor Cyan
        powershell -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "tools\build.ps1") $buildArgs
    }
    
    "install" {
        $installArgs = @()
        if ($Release) { $installArgs += "-Release" }
        if ($null -ne $ExtensionId -and $ExtensionId -ne "") {
            $installArgs += @("-ExtensionId", $ExtensionId)
        }
        if ($null -ne $GeckoId -and $GeckoId -ne "") {
            $installArgs += @("-GeckoId", $GeckoId)
        }

        Write-Host "Running host installer..." -ForegroundColor Cyan
        powershell -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "tools\install-host.ps1") $installArgs
    }
    
    "clean" {
        Write-Host "Cleaning build folders and developer profiles..." -ForegroundColor Cyan
        
        # 1. Clean cargo
        if (Test-Path (Join-Path $ProjectRoot "Cargo.toml")) {
            Write-Host "  Cleaning Rust target..." -ForegroundColor Yellow
            cargo clean
        }
        
        # 2. Clean extension dist
        $dist = Join-Path $ProjectRoot "extension\dist"
        if (Test-Path $dist) {
            Write-Host "  Removing extension dist..." -ForegroundColor Yellow
            Remove-Item $dist -Recurse -Force
        }
        
        # 3. Clean dev browser profiles
        $chromeProfile = Join-Path $ProjectRoot ".chrome-dev-profile"
        if (Test-Path $chromeProfile) {
            Write-Host "  Removing Chrome dev profile..." -ForegroundColor Yellow
            Remove-Item $chromeProfile -Recurse -Force
        }
        
        $firefoxProfile = Join-Path $ProjectRoot ".firefox-dev-profile"
        if (Test-Path $firefoxProfile) {
            Write-Host "  Removing Firefox dev profile..." -ForegroundColor Yellow
            Remove-Item $firefoxProfile -Recurse -Force
        }
        
        Write-Host "Cleanup completed!" -ForegroundColor Green
    }
    
    "run-chrome" {
        # 1. Build and install
        $buildArgs = @()
        if ($Release) { $buildArgs += "-Release" }
        powershell -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "tools\build.ps1") $buildArgs
        
        $installArgs = @()
        if ($Release) { $installArgs += "-Release" }
        if ($null -ne $ExtensionId -and $ExtensionId -ne "") {
            $installArgs += @("-ExtensionId", $ExtensionId)
        }
        powershell -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "tools\install-host.ps1") $installArgs

        # 2. Locate Chrome
        $chromePath = Get-ChromePath
        if ($null -eq $chromePath) {
            throw "Google Chrome executable not found. Please launch manually and load unpacked extension from extension\dist\chromium."
        }

        # 3. Launch Chrome with dev profile and loaded extension
        $extPath = Join-Path $ProjectRoot "extension\dist\chromium"
        $profilePath = Join-Path $ProjectRoot ".chrome-dev-profile"
        
        Write-Host "Launching Google Chrome with extension loaded..." -ForegroundColor Green
        Start-Process $chromePath -ArgumentList "--load-extension=""$extPath""", "--user-data-dir=""$profilePath""", "--no-first-run", "https://www.youtube.com"
    }
    
    "run-firefox" {
        # 1. Build and install
        $buildArgs = @()
        if ($Release) { $buildArgs += "-Release" }
        powershell -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "tools\build.ps1") $buildArgs
        
        $installArgs = @()
        if ($Release) { $installArgs += "-Release" }
        if ($null -ne $GeckoId -and $GeckoId -ne "") {
            $installArgs += @("-GeckoId", $GeckoId)
        }
        powershell -ExecutionPolicy Bypass -File (Join-Path $ProjectRoot "tools\install-host.ps1") $installArgs

        # 2. Locate Firefox
        $firefoxPath = Get-FirefoxPath
        if ($null -eq $firefoxPath) {
            throw "Mozilla Firefox executable not found. Please launch manually and load add-on from extension\dist\firefox."
        }

        # 3. Launch Firefox with a clean temporary profile and open about:debugging
        $profilePath = Join-Path $ProjectRoot ".firefox-dev-profile"
        
        Write-Host "Launching Mozilla Firefox..." -ForegroundColor Green
        Write-Host "Please navigate to 'about:debugging#/runtime/this-firefox' and load extension\dist\firefox\manifest.json as a temporary add-on." -ForegroundColor Yellow
        Start-Process $firefoxPath -ArgumentList "-no-remote", "-profile", """$profilePath""", "about:debugging#/runtime/this-firefox"
    }
}
