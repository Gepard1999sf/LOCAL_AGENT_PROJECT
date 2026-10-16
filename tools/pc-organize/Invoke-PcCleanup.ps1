# Invoke-PcCleanup.ps1 - frees disk space by deleting regenerable caches only.
# Dry run by default (prints what would be deleted and how much); -Apply deletes.
# Each target is a cache that the owning program recreates: nothing here is user data.
# Windows PowerShell 5.1; saved as UTF-8 with BOM.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcCleanup.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcCleanup.ps1 -Apply
#   ... -Skip npm,temp        skip targets by key
[CmdletBinding()]
param(
    [switch]$Apply,
    [string[]]$Skip = @(),
    [int]$TempOlderThanDays = 2
)

$ErrorActionPreference = 'Continue'

function Get-TreeBytes([string]$Path) {
    $bytes = [long]0
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $stack = New-Object 'System.Collections.Generic.Stack[string]'; $stack.Push($Path)
    while ($stack.Count -gt 0) {
        $d = $stack.Pop()
        try {
            $di = New-Object System.IO.DirectoryInfo $d
            foreach ($f in $di.EnumerateFiles()) { $bytes += $f.Length }
            foreach ($s in $di.EnumerateDirectories()) { if (($s.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) { $stack.Push($s.FullName) } }
        } catch { }
    }
    return $bytes
}

$targets = @(
    @{ Key = 'dxcache'; Name = 'Кэш шейдеров NVIDIA DXCache'; Path = "$env:LOCALAPPDATA\NVIDIA\DXCache"; Mode = 'contents' },
    @{ Key = 'glcache'; Name = 'Кэш шейдеров NVIDIA GLCache'; Path = "$env:LOCALAPPDATA\NVIDIA\GLCache"; Mode = 'contents' },
    @{ Key = 'wsl-crashes'; Name = 'Крэш-дампы WSL'; Path = "$env:LOCALAPPDATA\Temp\wsl-crashes"; Mode = 'contents' },
    @{ Key = 'docker-updates'; Name = 'Скачанные обновления Docker Desktop'; Path = "$env:LOCALAPPDATA\Temp\DockerDesktopUpdates"; Mode = 'contents' },
    @{ Key = 'unityhub'; Name = 'Загрузки Unity Hub (установщики редакторов)'; Path = "$env:APPDATA\UnityHub\downloads"; Mode = 'contents' },
    @{ Key = 'npm'; Name = 'Кэш npm'; Path = "$env:LOCALAPPDATA\npm-cache"; Mode = 'npm' },
    @{ Key = 'pip'; Name = 'Кэш pip'; Path = "$env:LOCALAPPDATA\pip\Cache"; Mode = 'contents' },
    @{ Key = 'puppeteer'; Name = 'Кэш Puppeteer (Chromium для скриптов)'; Path = "$env:USERPROFILE\.cache\puppeteer"; Mode = 'contents' },
    @{ Key = 'temp'; Name = "Temp: файлы старше $TempOlderThanDays дн."; Path = "$env:LOCALAPPDATA\Temp"; Mode = 'old' }
)

$total = [long]0
$rows = @()
foreach ($t in $targets) {
    if ($Skip -contains $t.Key) { continue }
    if (-not (Test-Path -LiteralPath $t.Path)) { continue }
    if ($t.Mode -eq 'old') {
        $files = Get-ChildItem -LiteralPath $t.Path -Recurse -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-$TempOlderThanDays) -and $_.FullName -notlike '*\wsl-crashes\*' -and $_.FullName -notlike '*\DockerDesktopUpdates\*' }
        $bytes = ($files | Measure-Object Length -Sum).Sum
    } else { $bytes = Get-TreeBytes $t.Path }
    $total += $bytes
    $rows += [pscustomobject]@{ Key = $t.Key; Target = $t.Name; GB = [math]::Round($bytes / 1GB, 2); Path = $t.Path }
}
$rows | Format-Table -AutoSize | Out-String -Width 160 | Write-Host
Write-Host ("Итого можно освободить: {0:N1} ГБ" -f ($total / 1GB)) -ForegroundColor Cyan

if (-not $Apply) { Write-Host 'Пробный прогон: ничего не удалено. Добавьте -Apply.' -ForegroundColor Yellow; return }

foreach ($t in $targets) {
    if ($Skip -contains $t.Key) { continue }
    if (-not (Test-Path -LiteralPath $t.Path)) { continue }
    Write-Host "Чищу: $($t.Name)" -ForegroundColor Cyan
    switch ($t.Mode) {
        'contents' { Get-ChildItem -LiteralPath $t.Path -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue }
        'npm' { & npm cache clean --force 2>&1 | Out-Null }
        'old' {
            Get-ChildItem -LiteralPath $t.Path -Recurse -File -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-$TempOlderThanDays) } |
                Remove-Item -Force -ErrorAction SilentlyContinue
            Get-ChildItem -LiteralPath $t.Path -Recurse -Directory -Force -ErrorAction SilentlyContinue | Sort-Object FullName -Descending |
                Where-Object { @(Get-ChildItem -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue).Count -eq 0 } |
                Remove-Item -Force -ErrorAction SilentlyContinue
        }
    }
}
$after = [long]0
foreach ($t in $targets) { if ($Skip -notcontains $t.Key -and (Test-Path -LiteralPath $t.Path)) { $after += Get-TreeBytes $t.Path } }
Write-Host ("Готово. Осталось в этих папках: {0:N1} ГБ (занятые файлы остаются)." -f ($after / 1GB)) -ForegroundColor Green
