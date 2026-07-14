# letterboxd-cli — instalador para Windows (source build via Scoop)
#
# Uso (PowerShell normal, no admin):
#   irm https://ser356.github.io/letterboxd-cli/install.ps1 | iex
#
# Instala Scoop (si falta), añade los buckets main + extras + ser356,
# instala letterboxd-cli (que trae VLC y rustup como deps) y compila el
# binario desde código fuente en tu máquina. Cero fricción para amigos.

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
    if ($buckets -notmatch "^\s*$Name\s") {
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

Ensure-Bucket -Name 'main'
Ensure-Bucket -Name 'extras'
Ensure-Bucket -Name 'ser356' -Url 'https://github.com/ser356/scoop-bucket'

Write-Host "==> Instalando letterboxd-cli (compila desde source, ~2-4 min)..." -ForegroundColor Cyan
scoop install ser356/letterboxd-cli

Write-Host ""
Write-Host "✅ Listo." -ForegroundColor Green
Write-Host ""
Write-Host "Cierra y vuelve a abrir PowerShell y ejecuta:"
Write-Host "  letterboxd-cli"
Write-Host ""
