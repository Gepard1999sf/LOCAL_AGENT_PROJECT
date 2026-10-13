# Copies the CodeWhale binary into src-tauri/binaries as the Tauri sidecar
# codewhale-app-server-<host triple>.exe, which `npm run bundle` packs into the installer.
# Usage: npm run sidecar [-- -Source C:\path\to\codewhale.exe]
# Written for Windows PowerShell 5.1; saved as UTF-8 with BOM so 5.1 reads the Cyrillic text.
param(
    [string]$Source
)

$ErrorActionPreference = 'Stop'

function Fail([string]$Message) {
    Write-Host $Message -ForegroundColor Red
    exit 1
}

$root = Split-Path -Parent $PSScriptRoot

# 1. The CodeWhale binary: -Source, then the npm install, then PATH.
$exe = $null
if ($Source) {
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        Fail "Файл не найден: $Source"
    }
    $exe = (Resolve-Path -LiteralPath $Source).Path
} else {
    if ($env:APPDATA) {
        $npmExe = Join-Path $env:APPDATA 'npm\node_modules\codewhale\bin\downloads\codewhale.exe'
        if (Test-Path -LiteralPath $npmExe -PathType Leaf) {
            $exe = $npmExe
        }
    }
    if (-not $exe) {
        # Only the real .exe: npm's codewhale.cmd / codewhale.ps1 shims cannot be a sidecar.
        $cmd = Get-Command 'codewhale.exe' -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($cmd) {
            $exe = $cmd.Path
        }
    }
}
if (-not $exe) {
    Fail ("Не найден codewhale.exe. Установите CodeWhale (npm i -g codewhale) " +
        "или укажите путь: npm run sidecar -- -Source C:\путь\к\codewhale.exe")
}

# 2. Host target triple: Tauri looks for binaries/codewhale-app-server-<triple>.exe.
if (-not (Get-Command 'rustc' -ErrorAction SilentlyContinue)) {
    Fail 'Не найден rustc. Установите Rust (https://rustup.rs) и откройте новое окно терминала.'
}
$hostLine = & rustc -vV | Where-Object { $_ -like 'host:*' } | Select-Object -First 1
if (-not $hostLine) {
    Fail 'Не удалось определить target triple: rustc -vV не вывел строку host.'
}
$triple = ($hostLine -replace '^host:\s*', '').Trim()

# 3. Copy.
$ext = ''
if ($triple -like '*windows*') {
    $ext = '.exe'
}
$binDir = Join-Path $root 'src-tauri\binaries'
if (-not (Test-Path -LiteralPath $binDir)) {
    New-Item -ItemType Directory -Path $binDir | Out-Null
}
$dest = Join-Path $binDir ('codewhale-app-server-' + $triple + $ext)
Write-Host "Источник:   $exe"
Write-Host "Назначение: $dest"
Copy-Item -LiteralPath $exe -Destination $dest -Force
$sizeMb = [math]::Round((Get-Item -LiteralPath $dest).Length / 1MB, 1)
Write-Host "Готово: sidecar для $triple ($sizeMb МБ)." -ForegroundColor Green
