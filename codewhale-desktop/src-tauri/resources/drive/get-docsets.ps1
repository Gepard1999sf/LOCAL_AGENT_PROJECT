# get-docsets.ps1 - downloads Zeal/Dash docsets straight from the Zeal feed (no Zeal install)
# into <DriveRoot>\docsets as <Name>.docset folders, which docset_search / docset_read read.
# Godot is not in the Zeal feed: Godot.docset is built locally from the class reference in
# <DriveRoot>\git_vault\gamedev\godot-docs by build-godot-docset.py (next to this script or in
# <DriveRoot>\bin; needs Python 3.11+).
# Windows PowerShell 5.1; needs tar.exe (built into Windows 10+). Saved as UTF-8 with BOM.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File get-docsets.ps1 -DriveRoot F:\AI_Drive
#   ... -Names Godot,"C++","Python 3","OpenGL 4"     (defaults; names/titles as the Zeal catalogue lists them)
#   ... -Names Godot -RebuildGodot                    (rebuild Godot.docset, e.g. after git pull in godot-docs)
param(
    [string]$DriveRoot = "D:\AI_Drive",
    [string[]]$Names = @("Godot", "C++", "Python 3", "OpenGL 4"),
    [switch]$RebuildGodot
)
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$docsets = Join-Path $DriveRoot "docsets"
New-Item -ItemType Directory -Force -Path $docsets | Out-Null
$here = $PSScriptRoot
if (-not $here) { $here = (Get-Location).Path }

function Have($cmd) { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

# `python` в PATH может оказаться заглушкой Microsoft Store (WindowsApps); лаунчер `py` надёжнее.
function Find-Python {
    if (Have "py") { return , @("py", "-3") }
    $p = Get-Command python -ErrorAction SilentlyContinue | Where-Object { $_.Source -notlike "*\WindowsApps\*" } | Select-Object -First 1
    if ($p) { return , @($p.Source) }
    return $null
}

function Build-GodotDocset {
    $folder = Join-Path $docsets "Godot.docset"
    if ((Test-Path (Join-Path $folder "Contents\Resources\docSet.dsidx")) -and -not $RebuildGodot) {
        Write-Host "  уже есть: $folder (пересобрать: -RebuildGodot)"
        return
    }
    $docs = Join-Path $DriveRoot "git_vault\gamedev\godot-docs"
    if (-not (Test-Path (Join-Path $docs "classes"))) {
        Write-Host "  нет исходников документации Godot: $docs" -ForegroundColor Yellow
        Write-Host "  склонируйте: git clone --depth 1 https://github.com/godotengine/godot-docs.git `"$docs`"" -ForegroundColor Yellow
        return
    }
    $script = @((Join-Path $here "build-godot-docset.py"), (Join-Path $DriveRoot "bin\build-godot-docset.py")) |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $script) {
        Write-Host "  не найден build-godot-docset.py (ни рядом со скриптом, ни в $DriveRoot\bin)" -ForegroundColor Yellow
        return
    }
    $py = Find-Python
    if (-not $py) {
        Write-Host "  python не найден в PATH: установите Python 3.11+ (python.org) и поставьте флажок Add to PATH" -ForegroundColor Yellow
        return
    }
    $exe = $py[0]
    $pyArgs = @($py | Select-Object -Skip 1)
    Write-Host "  собираю Godot.docset из $docs ($exe $($pyArgs -join ' ') $script)"
    # Python пишет предупреждения в stderr; в PowerShell 5.1 при Stop это стало бы исключением.
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $exe @pyArgs -I -X utf8 $script --docs $docs --out $docsets
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) { Write-Host "  сборка Godot.docset не удалась (код $code)" -ForegroundColor Yellow }
}

Write-Host "== Каталог докcетов Zeal" -ForegroundColor Cyan
$catalog = @()
try {
    $catalog = Invoke-RestMethod -Uri "https://api.zealdocs.org/v1/docsets" -UseBasicParsing
    Write-Host "  в каталоге $($catalog.Count) докcетов"
} catch {
    Write-Host "  каталог Zeal недоступен ($($_.Exception.Message))" -ForegroundColor Yellow
}

foreach ($want in $Names) {
    $entry = $catalog | Where-Object { $_.title -eq $want -or $_.name -eq $want -or $_.name -eq ($want -replace ' ', '_') } | Select-Object -First 1
    if (-not $entry) {
        if ($want -match '^godot') {
            Write-Host "  в каталоге Zeal нет ${want}: собираю из godot-docs"
            Build-GodotDocset
        } else {
            Write-Host "  не найден в каталоге: $want" -ForegroundColor Yellow
        }
        continue
    }
    $folder = Join-Path $docsets ("$($entry.name).docset")
    if (Test-Path (Join-Path $folder "Contents\Resources\docSet.dsidx")) { Write-Host "  уже есть: $folder"; continue }
    $url = "https://go.zealdocs.org/d/$($entry.sourceId)/$($entry.name)/latest"
    $tgz = Join-Path $docsets ("$($entry.name).tgz")
    Write-Host "  качаю $($entry.title) <- $url"
    try {
        Start-BitsTransfer -Source $url -Destination $tgz -DisplayName "docset $($entry.name)"
    } catch {
        Write-Host "  BITS не сработал ($($_.Exception.Message)), пробую Invoke-WebRequest" -ForegroundColor Yellow
        Invoke-WebRequest -Uri $url -OutFile $tgz -UseBasicParsing
    }
    Write-Host "  распаковываю $tgz"
    & tar.exe -xzf $tgz -C $docsets
    if ($LASTEXITCODE -ne 0) { throw "tar завершился с кодом $LASTEXITCODE для $tgz" }
    Remove-Item $tgz -Force
    $found = Get-ChildItem $docsets -Directory -Filter "*.docset" | Where-Object { Test-Path (Join-Path $_.FullName "Contents\Resources\docSet.dsidx") }
    Write-Host "  готово; докcетов на диске: $($found.Count)"
}

Write-Host "== Докcеты в $docsets" -ForegroundColor Cyan
Get-ChildItem $docsets -Directory -Filter "*.docset" | ForEach-Object {
    $idx = Join-Path $_.FullName "Contents\Resources\docSet.dsidx"
    $size = [math]::Round(((Get-ChildItem $_.FullName -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 0)
    "  {0,-28} {1,6} МБ  index={2}" -f $_.Name, $size, (Test-Path $idx)
}
