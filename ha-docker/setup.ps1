#Requires -Version 5.1
<#
.SYNOPSIS
    First-time setup and update script for Home Assistant + PyLoxone.
    Run this after every git pull to keep PyLoxone in sync.
#>

$ErrorActionPreference = "Stop"
$pyloxoneRepo  = "https://github.com/lkcauto/pyloxone.git"
$pyloxoneBranch = "claude/create-claude-md-QlfbO"
$pyloxoneLocal = "$env:USERPROFILE\ha-setup\pyloxone"
$scriptDir     = $PSScriptRoot
$ccDst         = "$scriptDir\ha-config\custom_components\loxone"

Write-Host "`n=== HA + PyLoxone Setup ===" -ForegroundColor Cyan

# 1. Check Docker
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    Write-Error "Docker not found. Install Docker Desktop: https://www.docker.com/products/docker-desktop/"
    exit 1
}
docker info 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Error "Docker Desktop is not running. Start it and try again."
    exit 1
}
Write-Host "[OK] Docker running" -ForegroundColor Green

# 2. Clone or update PyLoxone
if (Test-Path "$pyloxoneLocal\.git") {
    Write-Host "[..] Updating PyLoxone..." -ForegroundColor Yellow
    git -C $pyloxoneLocal fetch origin $pyloxoneBranch
    git -C $pyloxoneLocal checkout $pyloxoneBranch
    git -C $pyloxoneLocal pull origin $pyloxoneBranch
} else {
    Write-Host "[..] Cloning PyLoxone..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Path (Split-Path $pyloxoneLocal) -Force | Out-Null
    git clone --branch $pyloxoneBranch $pyloxoneRepo $pyloxoneLocal
}
Write-Host "[OK] PyLoxone at $pyloxoneLocal" -ForegroundColor Green

# 3. Copy custom_components into ha-config
$ccSrc = "$pyloxoneLocal\custom_components\loxone"
Write-Host "[..] Installing custom_components/loxone..." -ForegroundColor Yellow
if (Test-Path $ccDst) { Remove-Item $ccDst -Recurse -Force }
New-Item -ItemType Directory -Path $ccDst -Force | Out-Null
Copy-Item "$ccSrc\*" $ccDst -Recurse
Write-Host "[OK] custom_components/loxone installed" -ForegroundColor Green

# 4. Start Home Assistant
Write-Host "[..] Starting Home Assistant..." -ForegroundColor Yellow
Set-Location $scriptDir
docker compose up -d

Write-Host ""
Write-Host "Home Assistant is starting at http://localhost:8123" -ForegroundColor Green
Write-Host "First boot takes ~2 minutes. Refresh if the page isn't ready yet." -ForegroundColor Gray
Write-Host ""
