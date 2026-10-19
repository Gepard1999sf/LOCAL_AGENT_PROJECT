# === Развёртывание диска знаний для CodeWhale Desktop (init-ai-drive.ps1) ===
# Windows PowerShell 5.1. Запуск из корня съёмного диска или с любого места:
#   powershell -NoProfile -ExecutionPolicy Bypass -File init-ai-drive.ps1 -DriveRoot D:\AI_Drive
# Ключи: -SkipModels -SkipRepos -SkipZim -Math (докачать базу математики и физики), -Full (StackOverflow ~90 ГБ),
#        -KeepOllamaModels (не трогать OLLAMA_MODELS: модели остаются там, где лежат сейчас).
# Скрипт идемпотентный: уже скачанное не качает заново, прерванные загрузки докачиваются (BITS);
# новый выпуск .zim рядом со старым не качается (чтобы обновить книгу, удалите старый файл).
param(
    [string]$DriveRoot = "D:\AI_Drive",
    [switch]$SkipModels,
    [switch]$SkipRepos,
    [switch]$SkipZim,
    [switch]$Math,
    [switch]$Full,
    [switch]$KeepOllamaModels
)
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

function Step($text) { Write-Host ""; Write-Host "== $text" -ForegroundColor Cyan }
function Have($cmd) { return [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }

function Download($url, $dest) {
    if (Test-Path $dest) { Write-Host "  уже есть: $dest"; return }
    $tmp = "$dest.part"
    Write-Host "  качаю $url"
    try {
        # BITS докачивает после обрыва и не держит файл в памяти.
        Start-BitsTransfer -Source $url -Destination $tmp -DisplayName (Split-Path $dest -Leaf) -Description $url
        Move-Item $tmp $dest -Force
    } catch {
        Write-Host "  BITS не сработал ($($_.Exception.Message)), пробую Invoke-WebRequest" -ForegroundColor Yellow
        Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
        Move-Item $tmp $dest -Force
    }
}

# Листинги download.kiwix.org: каждый каталог запрашивается один раз за запуск, ошибка -> пустой листинг и предупреждение.
$script:listings = @{}
function Get-Listing($url) {
    if (-not $script:listings.ContainsKey($url)) {
        try {
            $script:listings[$url] = (Invoke-WebRequest -Uri $url -UseBasicParsing).Content
        } catch {
            Write-Host "  не удалось получить листинг $url ($($_.Exception.Message))" -ForegroundColor Yellow
            $script:listings[$url] = ""
        }
    }
    return $script:listings[$url]
}

# Самые свежие .zim из листинга, по одному на книгу. Имена Kiwix: <проект>_<язык>_<выборка>[_<вариант>]_<ГГГГ-ММ>.zim.
# Вариант mini пропускается; при одной дате полный/maxi лучше nopic.
function Find-NewestZims($listing, $namePattern) {
    $names = [regex]::Matches($listing, "href=`"($namePattern)`"") | ForEach-Object { $_.Groups[1].Value } |
        Where-Object { $_ -notmatch '_mini_' } | Select-Object -Unique
    $ranked = foreach ($n in $names) {
        $date = if ($n -match '_(\d{4}-\d{2})\.zim$') { $Matches[1] } else { "0000-00" }
        $rank = if ($n -match '_nopic_') { 1 } else { 2 }
        [pscustomobject]@{ Name = $n; Book = (ZimBook $n); Date = $date; Rank = $rank }
    }
    $ranked | Group-Object Book | ForEach-Object { $_.Group | Sort-Object Date, Rank | Select-Object -Last 1 -ExpandProperty Name }
}

# Имя книги без варианта и даты: math.stackexchange.com_en_all_2026-08.zim -> math.stackexchange.com_en_all.
function ZimBook($name) { return ($name -replace '(_maxi|_nopic|_mini)?_\d{4}-\d{2}\.zim$', '') }

# Качает .zim, если на диске нет никакой версии этой книги. Новый выпуск рядом со старым не качается
# (повторный запуск не тянет гигабайты молча); чтобы обновить, удалите старый файл и запустите скрипт снова.
function DownloadZim($dirUrl, $name, $zimDir) {
    $book = ZimBook $name
    $local = Get-ChildItem $zimDir -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '\.zim$' -and (ZimBook $_.Name) -eq $book } | Select-Object -First 1
    if ($local -and $local.Name -ne $name) {
        Write-Host "  уже есть $($local.Name); на сервере $name (для обновления удалите старый файл)" -ForegroundColor Yellow
        return
    }
    Download "$dirUrl$name" (Join-Path $zimDir $name)
}

function Clone($url, $dir) {
    if (Test-Path (Join-Path $dir ".git")) { Write-Host "  уже есть: $dir"; return }
    Write-Host "  клонирую $url"
    & git clone --depth 1 --single-branch $url $dir
    if ($LASTEXITCODE -ne 0) { throw "git clone завершился с кодом $LASTEXITCODE" }
}

if (-not (Test-Path (Split-Path $DriveRoot -Qualifier))) { throw "Диск $(Split-Path $DriveRoot -Qualifier) не подключён" }

Step "Структура папок в $DriveRoot"
$dirs = @("models", "docsets", "git_vault\gamedev", "git_vault\physics", "git_vault\fullstack", "kiwix_zim", "math_physics", "vectordb", "bin", "index", "AI_Workflows", "AI_ComfyModels")
foreach ($d in $dirs) { New-Item -ItemType Directory -Force -Path (Join-Path $DriveRoot $d) | Out-Null }
Write-Host "  готово"

if ($KeepOllamaModels) {
    Step "Хранилище моделей Ollama не меняется (-KeepOllamaModels)"
    $current = [Environment]::GetEnvironmentVariable("OLLAMA_MODELS", "User")
    if ($current) { Write-Host "  OLLAMA_MODELS=$current" } else { Write-Host "  OLLAMA_MODELS не задана: модели в %USERPROFILE%\.ollama\models" }
} else {
    Step "Хранилище моделей Ollama -> $DriveRoot\models (переменная пользователя OLLAMA_MODELS)"
    $models = Join-Path $DriveRoot "models"
    [Environment]::SetEnvironmentVariable("OLLAMA_MODELS", $models, "User")
    $env:OLLAMA_MODELS = $models
    Write-Host "  OLLAMA_MODELS=$models (перезапустите Ollama из трея, чтобы применилось)"
}

if (-not $SkipModels) {
    Step "Золотой набор моделей (8 ГБ VRAM + 32 ГБ RAM)"
    if (-not (Have "ollama")) {
        Write-Host "  ollama не найдена в PATH: установите с https://ollama.com/download и запустите скрипт снова" -ForegroundColor Yellow
    } else {
        # Быстрый кодер, главный MoE-агент, кодер для сложных правок, архитектор, черновик, эмбеддинги.
        $golden = @("qwen2.5-coder:7b", "qwen3:30b-a3b", "qwen2.5-coder:14b", "deepseek-r1:14b", "qwen2.5-coder:1.5b", "bge-m3")
        foreach ($m in $golden) {
            Write-Host "  ollama pull $m"
            & ollama pull $m
            if ($LASTEXITCODE -ne 0) { Write-Host "  не удалось скачать $m (тег изменился? проверьте ollama.com/library)" -ForegroundColor Yellow }
        }
    }
}

if (-not $SkipRepos) {
    Step "Эталонные репозитории (git clone --depth 1)"
    if (-not (Have "git")) {
        Write-Host "  git не найден: установите Git for Windows" -ForegroundColor Yellow
    } else {
        $gamedev = Join-Path $DriveRoot "git_vault\gamedev"
        Clone "https://github.com/godotengine/godot.git" (Join-Path $gamedev "godot")
        Clone "https://github.com/godotengine/godot-demo-projects.git" (Join-Path $gamedev "godot-demo-projects")
        Clone "https://github.com/godotengine/godot-docs.git" (Join-Path $gamedev "godot-docs")
        Clone "https://github.com/GDQuest/godot-4-3d-third-person-controller.git" (Join-Path $gamedev "godot-4-3d-third-person-controller")
        if ($Math) {
            $physics = Join-Path $DriveRoot "git_vault\physics"
            Clone "https://github.com/jrouwe/JoltPhysics.git" (Join-Path $physics "JoltPhysics")
            Clone "https://github.com/erincatto/box2d.git" (Join-Path $physics "box2d")
            Clone "https://github.com/matthias-research/pages.git" (Join-Path $physics "ten-minute-physics")
            Clone "https://github.com/g-truc/glm.git" (Join-Path $physics "glm")
        }
    }
}

if (-not $SkipZim) {
    Step "Дампы StackExchange для Kiwix (.zim)"
    $zim = Join-Path $DriveRoot "kiwix_zim"
    $base = "https://download.kiwix.org/zim/stack_exchange"
    # Имена файлов на сервере содержат дату выпуска; берём актуальные из листинга.
    $listing = Get-Listing "$base/"
    $wanted = @("gamedev.stackexchange.com_en_all", "blender.stackexchange.com_en_all")
    if ($Math) { $wanted += @("math.stackexchange.com_en_all", "mathoverflow.net_en_all", "physics.stackexchange.com_en_all", "scicomp.stackexchange.com_en_all") }
    if ($Full) { $wanted += @("stackoverflow.com_en_all") }
    foreach ($w in $wanted) {
        $match = [regex]::Matches($listing, "href=`"($w[^`"]*\.zim)`"") | ForEach-Object { $_.Groups[1].Value } | Sort-Object | Select-Object -Last 1
        if (-not $match) { Write-Host "  в листинге нет $w — откройте $base/ и скачайте вручную" -ForegroundColor Yellow; continue }
        DownloadZim "$base/" $match $zim
    }
    if ($Math) {
        Step "Учебники физики и математики для Kiwix (OpenStax, иначе LibreTexts)"
        # OpenStax в Kiwix больше не публикуется (нет ни в zim/other/, ни в каталоге library.kiwix.org, проверено в 2026-10),
        # поэтому для каждого предмета сначала ищем OpenStax, затем LibreTexts. Только английские книги, самые свежие.
        $textbooks = @(
            @{ Subject = "физика"; Candidates = @(
                @{ Label = "OpenStax University/College Physics"; Dir = "other"; Pattern = 'openstax[^"/]*_en_[^"/]*physics[^"/]*\.zim' },
                @{ Label = "LibreTexts Physics"; Dir = "libretexts"; Pattern = 'libretexts\.org_en_phys(_maxi|_nopic)?_\d{4}-\d{2}\.zim' }) },
            @{ Subject = "математика"; Candidates = @(
                @{ Label = "OpenStax Calculus"; Dir = "other"; Pattern = 'openstax[^"/]*_en_[^"/]*calculus[^"/]*\.zim' },
                @{ Label = "LibreTexts Mathematics"; Dir = "libretexts"; Pattern = 'libretexts\.org_en_math(_maxi|_nopic)?_\d{4}-\d{2}\.zim' }) }
        )
        foreach ($t in $textbooks) {
            $found = @()
            foreach ($c in $t.Candidates) {
                $dirUrl = "https://download.kiwix.org/zim/$($c.Dir)/"
                $found = @(Find-NewestZims (Get-Listing $dirUrl) $c.Pattern)
                if ($found.Count -gt 0) { Write-Host "  $($t.Subject): $($c.Label)"; break }
                Write-Host "  $($t.Subject): $($c.Label) нет в $dirUrl" -ForegroundColor Yellow
            }
            if ($found.Count -eq 0) {
                Write-Host "  $($t.Subject): учебник не скачан. Найдите .zim на https://library.kiwix.org и положите в $zim" -ForegroundColor Yellow
                continue
            }
            foreach ($name in $found) { DownloadZim $dirUrl $name $zim }
        }
    }

    Step "kiwix-serve (kiwix-tools для Windows) -> $DriveRoot\bin"
    $bin = Join-Path $DriveRoot "bin"
    if (-not (Get-ChildItem $bin -Recurse -Filter "kiwix-serve.exe" -ErrorAction SilentlyContinue)) {
        try {
            $tools = (Invoke-WebRequest -Uri "https://download.kiwix.org/release/kiwix-tools/" -UseBasicParsing).Content
            # Сборки win-x86_64 (с 3.8.0) и win-i686: берём самую новую версию, при равной версии x86_64.
            # Строковая сортировка тут не годится: она выбирала i686-3.7.0-2 вместо x86_64-3.8.1.
            $zipName = [regex]::Matches($tools, "href=`"(kiwix-tools_win-(x86_64|i686)-(\d+(?:\.\d+)+)(?:-(\d+))?\.zip)`"") | ForEach-Object {
                [pscustomobject]@{ Name = $_.Groups[1].Value; Version = [version]$_.Groups[3].Value; Build = [int]("0" + $_.Groups[4].Value); X64 = ($_.Groups[2].Value -eq "x86_64") }
            } | Sort-Object Version, Build, X64 | Select-Object -Last 1 -ExpandProperty Name
            if ($zipName) {
                $zipPath = Join-Path $bin $zipName
                Download "https://download.kiwix.org/release/kiwix-tools/$zipName" $zipPath
                Expand-Archive -Path $zipPath -DestinationPath $bin -Force
            } else { Write-Host "  не нашёл kiwix-tools_win-*.zip в листинге" -ForegroundColor Yellow }
        } catch { Write-Host "  kiwix-tools не скачан: $($_.Exception.Message)" -ForegroundColor Yellow }
    } else { Write-Host "  уже есть" }
}

Step "SymPy для символьной математики"
# `python` в PATH может оказаться заглушкой Microsoft Store; лаунчер `py` надёжнее.
$python = $null
if (Have "py") { $python = "py" } elseif (Have "python") { $python = "python" }
if ($python) {
    & $python -c "import sympy" 2>$null
    if ($LASTEXITCODE -ne 0) { & $python -m pip install --user sympy } else { Write-Host "  уже установлен ($python)" }
} else { Write-Host "  python не найден в PATH: установите Python 3 (python.org) и поставьте флажок Add to PATH" -ForegroundColor Yellow }

Step "Готово"
Write-Host "  Дальше в CodeWhale Desktop: Диск знаний -> Подключить MCP, Установить скиллы, Построить индекс, Запустить Kiwix."
Write-Host "  Докcеты Zeal кладите в $DriveRoot\docsets (папки *.docset)."
