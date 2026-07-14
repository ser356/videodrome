# videodrome — installer para Windows (Scoop, binario prebuilt)
#
# Uso (PowerShell normal, NO admin):
#   irm https://ser356.github.io/videodrome/install.ps1 | iex
#
# Instala Scoop (si falta), añade los buckets extras (VLC) + ser356
# (videodrome) e instala el paquete. El binario ya viene compilado —
# no hace falta Rust ni node en tu máquina. ~30 segundos.

$ErrorActionPreference = 'Stop'

if ([Security.Principal.WindowsPrincipal] `
    [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole] 'Administrator') {
    Write-Host "❌ No ejecutes este script como Administrador." -ForegroundColor Red
    Write-Host "   Scoop se instala en tu perfil de usuario, no en el sistema."
    exit 1
}

Write-Host "==> Comprobando ExecutionPolicy..." -ForegroundColor Cyan
if ((Get-ExecutionPolicy -Scope CurrentUser) -in @('Restricted','Undefined')) {
    Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser -Force
    Write-Host "   ExecutionPolicy establecida a RemoteSigned (CurrentUser)."
} else {
    Write-Host "   OK."
}

if (-not (Get-Command scoop -ErrorAction SilentlyContinue)) {
    Write-Host "==> Instalando Scoop..." -ForegroundColor Cyan
    Invoke-Expression (New-Object System.Net.WebClient).DownloadString('https://get.scoop.sh')
    $env:PATH = "$env:USERPROFILE\scoop\shims;$env:PATH"
} else {
    Write-Host "==> Scoop ya instalado." -ForegroundColor Cyan
}

function Ensure-Bucket {
    param([string]$Name, [string]$Url = '')
    $buckets = scoop bucket list 2>$null | Out-String
    if ($buckets -notmatch "(?m)^\s*$Name\s") {
        Write-Host "==> Añadiendo bucket $Name..." -ForegroundColor Cyan
        if ($Url) {
            scoop bucket add $Name $Url
        } else {
            scoop bucket add $Name
        }
    } else {
        Write-Host "==> Bucket $Name ya presente." -ForegroundColor Cyan
    }
}

Ensure-Bucket -Name 'extras'
Ensure-Bucket -Name 'ser356' -Url 'https://github.com/ser356/scoop-bucket'

Write-Host "==> Instalando videodrome (binario prebuilt, ~30s)..." -ForegroundColor Cyan
scoop install ser356/videodrome

Write-Host ""
Write-Host "✅ Listo." -ForegroundColor Green
Write-Host ""
Write-Host "  · Doble click en Start Menu (busca 'Videodrome') → GUI"
Write-Host "  · videodrome recommend                              → CLI"
Write-Host "  · videodrome tui                                    → TUI en terminal"
Write-Host ""
