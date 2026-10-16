# === Развёртывание диска знаний для CodeWhale Desktop (init-ai-drive.ps1) ===
# Windows PowerShell 5.1. Запуск из корня съёмного диска или с любого места:
#   powershell -NoProfile -ExecutionPolicy Bypass -File init-ai-drive.ps1 -DriveRoot D:\AI_Drive
# Ключи: -SkipModels -SkipRepos -SkipZim -Math (докачать базу математики и физики), -Full (StackOverflow ~90 ГБ).
# Скрипт идемпотентный: уже скачанное не качает заново, прерванные загрузки докачиваются (BITS).
param(
    [string]$DriveRoot = "D:\AI_Drive",
    [switch]$SkipModels,
    [switch]$SkipRepos,
    [switch]$SkipZim,
    [switch]$Math,
    [switch]$Full
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

Step "Хранилище моделей Ollama -> $DriveRoot\models (переменная пользователя OLLAMA_MODELS)"
$models = Join-Path $DriveRoot "models"
[Environment]::SetEnvironmentVariable("OLLAMA_MODELS", $models, "User")
$env:OLLAMA_MODELS = $models
Write-Host "  OLLAMA_MODELS=$models (перезапустите Ollama из трея, чтобы применилось)"

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
    try {
        $listing = (Invoke-WebRequest -Uri "$base/" -UseBasicParsing).Content
    } catch {
        $listing = ""
        Write-Host "  не удалось получить листинг $base ($($_.Exception.Message))" -ForegroundColor Yellow
    }
    $wanted = @("gamedev.stackexchange.com_en_all", "blender.stackexchange.com_en_all")
    if ($Math) { $wanted += @("math.stackexchange.com_en_all", "mathoverflow.net_en_all", "physics.stackexchange.com_en_all", "scicomp.stackexchange.com_en_all") }
    if ($Full) { $wanted += @("stackoverflow.com_en_all") }
    foreach ($w in $wanted) {
        $match = [regex]::Matches($listing, "href=`"($w[^`"]*\.zim)`"") | ForEach-Object { $_.Groups[1].Value } | Sort-Object | Select-Object -Last 1
        if (-not $match) { Write-Host "  в листинге нет $w — откройте $base/ и скачайте вручную" -ForegroundColor Yellow; continue }
        Download "$base/$match" (Join-Path $zim $match)
    }
    if ($Math) {
        Step "Учебники OpenStax (Kiwix, папка other)"
        try {
            $other = (Invoke-WebRequest -Uri "https://download.kiwix.org/zim/other/" -UseBasicParsing).Content
            $m = [regex]::Matches($other, "href=`"(openstax[^`"]*\.zim)`"") | ForEach-Object { $_.Groups[1].Value } | Sort-Object | Select-Object -Last 1
            if ($m) { Download "https://download.kiwix.org/zim/other/$m" (Join-Path $zim $m) }
        } catch { Write-Host "  листинг other/ недоступен" -ForegroundColor Yellow }
    }

    Step "kiwix-serve (kiwix-tools для Windows) -> $DriveRoot\bin"
    $bin = Join-Path $DriveRoot "bin"
    if (-not (Get-ChildItem $bin -Recurse -Filter "kiwix-serve.exe" -ErrorAction SilentlyContinue)) {
        try {
            $tools = (Invoke-WebRequest -Uri "https://download.kiwix.org/release/kiwix-tools/" -UseBasicParsing).Content
            $zipName = [regex]::Matches($tools, "href=`"(kiwix-tools_win-i686-[^`"]*\.zip)`"") | ForEach-Object { $_.Groups[1].Value } | Sort-Object | Select-Object -Last 1
            if ($zipName) {
                $zipPath = Join-Path $bin $zipName
                Download "https://download.kiwix.org/release/kiwix-tools/$zipName" $zipPath
                Expand-Archive -Path $zipPath -DestinationPath $bin -Force
            } else { Write-Host "  не нашёл kiwix-tools_win-i686-*.zip в листинге" -ForegroundColor Yellow }
        } catch { Write-Host "  kiwix-tools не скачан: $($_.Exception.Message)" -ForegroundColor Yellow }
    } else { Write-Host "  уже есть" }
}

Step "SymPy для символьной математики"
if (Have "python") {
    & python -c "import sympy" 2>$null
    if ($LASTEXITCODE -ne 0) { & python -m pip install --user sympy } else { Write-Host "  уже установлен" }
} else { Write-Host "  python не найден в PATH: установите Python 3 (python.org) и поставьте флажок Add to PATH" -ForegroundColor Yellow }

Step "Готово"
Write-Host "  Дальше в CodeWhale Desktop: Диск знаний -> Подключить MCP, Установить скиллы, Построить индекс, Запустить Kiwix."
Write-Host "  Докcеты Zeal кладите в $DriveRoot\docsets (папки *.docset)."
