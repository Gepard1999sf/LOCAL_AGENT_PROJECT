# Invoke-PcAudit.ps1 - read-only audit of a Windows PC: disks, folders, caches,
# installed programs, startup, games/engines, Downloads, Desktop, projects, big files.
# Writes PC-Audit-<date>.md and PC-Audit-<date>.json to <Desktop>\PC-Audit (or -OutDir).
# Changes nothing on the machine. Windows PowerShell 5.1; saved as UTF-8 with BOM.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcAudit.ps1
#   ... -Quick            skip the slow parts (big-file scan, deep sizes of Program Files)
#   ... -BigFileMB 300    threshold for the big-file list (default 500)
#   ... -ProjectRoots F:\PROJECTS,D:\MY_UNITY_PROJECTS   extra folders to scan for projects
[CmdletBinding()]
param(
    [string]$OutDir,
    [switch]$Quick,
    [int]$BigFileMB = 500,
    [int]$TopN = 30,
    [string[]]$ProjectRoots = @()
)

$ErrorActionPreference = 'Continue'
$script:Started = Get-Date
$stamp = $script:Started.ToString('yyyy-MM-dd_HH-mm')
$user = $env:USERNAME
$home_ = $env:USERPROFILE
if (-not $OutDir) { $OutDir = Join-Path ([Environment]::GetFolderPath('Desktop')) 'PC-Audit' }
if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }

# ---------------------------------------------------------------- helpers

function Format-Gb([double]$bytes, [int]$digits = 2) { [math]::Round($bytes / 1GB, $digits) }
function Format-Mb([double]$bytes, [int]$digits = 1) { [math]::Round($bytes / 1MB, $digits) }

# Fast recursive size: .NET enumeration, skips reparse points, swallows access errors per folder.
function Get-TreeStats([string]$Path) {
    $bytes = [long]0; $files = [long]0; $dirs = [long]0
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{ Path = $Path; Bytes = 0; Files = 0; Dirs = 0; Missing = $true }
    }
    $stack = New-Object 'System.Collections.Generic.Stack[string]'
    $stack.Push($Path)
    while ($stack.Count -gt 0) {
        $d = $stack.Pop()
        try {
            $di = New-Object System.IO.DirectoryInfo $d
            foreach ($f in $di.EnumerateFiles()) { $bytes += $f.Length; $files++ }
            foreach ($s in $di.EnumerateDirectories()) {
                if (($s.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) { $stack.Push($s.FullName); $dirs++ }
            }
        } catch { }
    }
    [pscustomobject]@{ Path = $Path; Bytes = $bytes; Files = $files; Dirs = $dirs; Missing = $false }
}

# Walk a tree and collect files above a size threshold.
function Find-BigFiles([string]$Path, [long]$MinBytes, [string[]]$SkipNames) {
    $found = New-Object System.Collections.Generic.List[object]
    if (-not (Test-Path -LiteralPath $Path)) { return $found }
    $stack = New-Object 'System.Collections.Generic.Stack[string]'
    $stack.Push($Path)
    while ($stack.Count -gt 0) {
        $d = $stack.Pop()
        try {
            $di = New-Object System.IO.DirectoryInfo $d
            foreach ($f in $di.EnumerateFiles()) {
                if ($f.Length -ge $MinBytes) {
                    $found.Add([pscustomobject]@{ Path = $f.FullName; GB = Format-Gb $f.Length; Modified = $f.LastWriteTime.ToString('yyyy-MM-dd') })
                }
            }
            foreach ($s in $di.EnumerateDirectories()) {
                if (($s.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
                if ($SkipNames -contains $s.Name) { continue }
                $stack.Push($s.FullName)
            }
        } catch { }
    }
    return $found
}

function Get-ChildStats([string]$Path, [int]$Top = 0) {
    $rows = @()
    if (-not (Test-Path -LiteralPath $Path)) { return $rows }
    foreach ($d in (Get-ChildItem -LiteralPath $Path -Directory -Force -ErrorAction SilentlyContinue)) {
        if (($d.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
        if ($d.Name -in @('$Recycle.Bin', '$RECYCLE.BIN', 'System Volume Information', 'Recovery', 'Documents and Settings')) { continue }
        $s = Get-TreeStats $d.FullName
        $rows += [pscustomobject]@{ Name = $d.Name; Path = $d.FullName; GB = Format-Gb $s.Bytes; Files = $s.Files; Modified = $d.LastWriteTime.ToString('yyyy-MM-dd') }
    }
    $rows = $rows | Sort-Object GB -Descending
    if ($Top -gt 0) { $rows = $rows | Select-Object -First $Top }
    return $rows
}

function Test-ProjectType([string]$Path) {
    $has = { param($n) Test-Path -LiteralPath (Join-Path $Path $n) }
    $types = @()
    if (& $has '.git') { $types += 'git' }
    if (& $has 'project.godot') { $types += 'Godot' }
    if ((& $has 'Assets') -and (& $has 'ProjectSettings')) { $types += 'Unity' }
    $files = @(Get-ChildItem -LiteralPath $Path -File -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Extension.ToLowerInvariant() })
    if ($files -contains '.uproject' -or $files -contains '.uefnproject') { $types += 'Unreal' }
    if (& $has 'Cargo.toml') { $types += 'Rust' }
    if (& $has 'package.json') { $types += 'Node' }
    if ((& $has 'pyproject.toml') -or (& $has 'requirements.txt')) { $types += 'Python' }
    if ((& $has 'src-tauri')) { $types += 'Tauri' }
    if (($files -contains '.sln' -or $files -contains '.slnx' -or $files -contains '.csproj') -and $types -notcontains 'Unity') { $types += '.NET' }
    if (& $has 'docker-compose.yml') { $types += 'Docker' }
    return ($types -join '+')
}

function Get-BuildDirBytes([string]$Path) {
    # Size of regenerable build/dependency folders inside a project (depth <= 2).
    $names = @('node_modules', 'target', '.godot', 'Library', 'Temp', 'obj', 'bin', 'dist', 'build', '.venv', 'venv', '__pycache__', 'Intermediate', 'DerivedDataCache', 'Saved')
    $bytes = [long]0
    foreach ($depth in 0..2) {
        $pattern = $Path
        foreach ($i in 1..$depth) { $pattern = Join-Path $pattern '*' }
        if ($depth -eq 0) { $cands = @((Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue)) } else { $cands = Get-ChildItem -Path $pattern -Directory -Force -ErrorAction SilentlyContinue }
        foreach ($c in $cands) {
            foreach ($n in $names) {
                $p = Join-Path $c.FullName $n
                if (Test-Path -LiteralPath $p) { $bytes += (Get-TreeStats $p).Bytes }
            }
        }
    }
    return $bytes
}

function Get-DownloadCategory([System.IO.FileInfo]$f) {
    $ext = $f.Extension.ToLowerInvariant()
    $n = $f.Name
    switch -Regex ($ext) {
        '^\.(exe|msi|msix|appx)$' { return 'Установщики' }
        '^\.(zip|rar|7z|tar|gz|tgz|xz|bz2)$' {
            if ($n -match '\.sql\.gz$') { return 'Дампы БД' }
            return 'Архивы'
        }
        '^\.(docx|doc|odt|rtf|txt|md|pdf|xlsx|xls|csv|pptx)$' { return 'Документы' }
        '^\.(png|jpg|jpeg|webp|bmp|svg|clip|psd)$' { return 'Изображения' }
        '^\.gif$' { return 'GIF' }
        '^\.(mp4|mov|mkv|avi|webm)$' { return 'Видео' }
        '^\.(mp3|wav|flac|ogg)$' { return 'Аудио' }
        '^\.torrent$' { return 'Торренты' }
        '^\.(html|htm)$' { return 'HTML' }
        '^\.(sql|db|sqlite)$' { return 'Дампы БД' }
        '^\.(conf|ini|json|yaml|yml|toml|rdp|cfg)$' { return 'Конфиги' }
        '^\.(blend|fbx|glb|gltf|obj)$' { return '3D' }
        default { return 'Прочее' }
    }
}

# ---------------------------------------------------------------- 1. system

Write-Host "[1/12] Система..." -ForegroundColor Cyan
$os = Get-CimInstance Win32_OperatingSystem
$cs = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$gpus = Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) ($([math]::Round($_.AdapterRAM/1GB,1)) GB)" }
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$system = [ordered]@{
    Computer = $cs.Name; Model = "$($cs.Manufacturer) $($cs.Model)"; User = $user
    OS = "$($os.Caption) $($os.Version) (build $($os.BuildNumber))"
    InstallDate = $os.InstallDate.ToString('yyyy-MM-dd'); LastBoot = $os.LastBootUpTime.ToString('yyyy-MM-dd HH:mm')
    CPU = $cpu.Name; RamGB = [math]::Round($cs.TotalPhysicalMemory / 1GB, 1); GPU = ($gpus -join '; ')
    Admin = $isAdmin; AuditStarted = $script:Started.ToString('yyyy-MM-dd HH:mm')
}

# ---------------------------------------------------------------- 2. disks

Write-Host "[2/12] Диски..." -ForegroundColor Cyan
$physical = Get-PhysicalDisk -ErrorAction SilentlyContinue | ForEach-Object { [pscustomobject]@{ Disk = $_.DeviceId; Model = $_.FriendlyName; Type = "$($_.MediaType)/$($_.BusType)"; GB = [math]::Round($_.Size / 1GB); Health = "$($_.HealthStatus)" } }
$volumes = Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object {
    $part = Get-Partition -DriveLetter $_.DeviceID.TrimEnd(':') -ErrorAction SilentlyContinue
    [pscustomobject]@{ Drive = $_.DeviceID; Label = $_.VolumeName; FS = $_.FileSystem; Disk = $(if ($part) { $part.DiskNumber } else { '' })
        TotalGB = Format-Gb $_.Size 0; UsedGB = Format-Gb ($_.Size - $_.FreeSpace) 0; FreeGB = Format-Gb $_.FreeSpace 0; FreePct = [math]::Round(100 * $_.FreeSpace / $_.Size) }
}
$sysFiles = @()
foreach ($v in $volumes) {
    foreach ($n in 'hiberfil.sys', 'pagefile.sys', 'swapfile.sys') {
        $p = Join-Path "$($v.Drive)\" $n
        $fi = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
        if ($fi) { $sysFiles += [pscustomobject]@{ File = $p; GB = Format-Gb $fi.Length } }
    }
}

# ---------------------------------------------------------------- 3. top-level folders per drive

Write-Host "[3/12] Папки верхнего уровня на каждом диске (это занимает несколько минут)..." -ForegroundColor Cyan
$driveTop = [ordered]@{}
foreach ($v in $volumes) {
    $root = "$($v.Drive)\"
    $rows = @()
    foreach ($d in (Get-ChildItem -LiteralPath $root -Directory -Force -ErrorAction SilentlyContinue)) {
        if ($d.Name -in @('$Recycle.Bin', '$RECYCLE.BIN', 'System Volume Information', 'Recovery', 'Documents and Settings')) { continue }
        if (($d.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
        if ($Quick -and $d.Name -in @('Windows', 'Program Files', 'Program Files (x86)', 'ProgramData', 'Users')) {
            $rows += [pscustomobject]@{ Name = $d.Name; GB = $null; Files = $null; Modified = $d.LastWriteTime.ToString('yyyy-MM-dd') }; continue
        }
        $s = Get-TreeStats $d.FullName
        $rows += [pscustomobject]@{ Name = $d.Name; GB = Format-Gb $s.Bytes; Files = $s.Files; Modified = $d.LastWriteTime.ToString('yyyy-MM-dd') }
    }
    foreach ($f in (Get-ChildItem -LiteralPath $root -File -Force -ErrorAction SilentlyContinue)) {
        if ($f.Name -in @('hiberfil.sys', 'pagefile.sys', 'swapfile.sys', 'DumpStack.log', 'DumpStack.log.tmp', 'desktop.ini')) { continue }
        $rows += [pscustomobject]@{ Name = "$($f.Name) (файл)"; GB = Format-Gb $f.Length; Files = 1; Modified = $f.LastWriteTime.ToString('yyyy-MM-dd') }
    }
    $driveTop[$v.Drive] = @($rows | Sort-Object GB -Descending)
}

# ---------------------------------------------------------------- 4. user profile

Write-Host "[4/12] Профиль пользователя..." -ForegroundColor Cyan
$profileRows = @()
foreach ($d in (Get-ChildItem -LiteralPath $home_ -Directory -Force -ErrorAction SilentlyContinue)) {
    if (($d.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
    if ($d.Name -eq 'AppData') { continue }
    $s = Get-TreeStats $d.FullName
    $kind = Test-ProjectType $d.FullName
    $profileRows += [pscustomobject]@{ Name = $d.Name; GB = Format-Gb $s.Bytes; Files = $s.Files; Kind = $kind; Modified = $d.LastWriteTime.ToString('yyyy-MM-dd') }
}
$profileRows = $profileRows | Sort-Object GB -Descending

# ---------------------------------------------------------------- 5. AppData and caches

Write-Host "[5/12] AppData и кэши..." -ForegroundColor Cyan
$appData = @()
foreach ($base in "$home_\AppData\Local", "$home_\AppData\Roaming", "$home_\AppData\LocalLow") {
    foreach ($r in (Get-ChildStats $base)) { $appData += [pscustomobject]@{ Path = $r.Path; GB = $r.GB; Files = $r.Files } }
}
foreach ($d in (Get-ChildItem -LiteralPath $home_ -Directory -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -like '.*' })) {
    $s = Get-TreeStats $d.FullName
    $appData += [pscustomobject]@{ Path = $d.FullName; GB = Format-Gb $s.Bytes; Files = $s.Files }
}
$appData = $appData | Sort-Object GB -Descending

$cacheDefs = @(
    @{ Name = 'Кэш шейдеров NVIDIA (DXCache)'; Path = "$env:LOCALAPPDATA\NVIDIA\DXCache"; Safe = 'да'; How = 'удалить папку; игры пересоздадут кэш (первый запуск чуть дольше)' },
    @{ Name = 'Кэш шейдеров NVIDIA (GLCache)'; Path = "$env:LOCALAPPDATA\NVIDIA\GLCache"; Safe = 'да'; How = 'удалить папку' },
    @{ Name = 'Temp пользователя'; Path = "$env:LOCALAPPDATA\Temp"; Safe = 'да (кроме открытых файлов)'; How = 'Параметры → Система → Память → Временные файлы, или удалить вручную файлы старше недели' },
    @{ Name = 'Крэш-дампы WSL'; Path = "$env:LOCALAPPDATA\Temp\wsl-crashes"; Safe = 'да'; How = 'удалить папку' },
    @{ Name = 'Кэш npm'; Path = "$env:LOCALAPPDATA\npm-cache"; Safe = 'да'; How = 'npm cache clean --force' },
    @{ Name = 'Кэш pip'; Path = "$env:LOCALAPPDATA\pip\Cache"; Safe = 'да'; How = 'pip cache purge' },
    @{ Name = 'Кэш uv'; Path = "$env:LOCALAPPDATA\uv\cache"; Safe = 'да'; How = 'uv cache clean' },
    @{ Name = 'Реестр crates cargo'; Path = "$home_\.cargo\registry"; Safe = 'да'; How = 'удалить registry\cache и registry\src; cargo скачает заново' },
    @{ Name = 'Тулчейны rustup'; Path = "$home_\.rustup\toolchains"; Safe = 'частично'; How = 'rustup toolchain list; удалить неиспользуемые: rustup toolchain uninstall <имя>' },
    @{ Name = 'Загрузки Unity Hub'; Path = "$env:APPDATA\UnityHub\downloads"; Safe = 'да'; How = 'удалить содержимое (установщики редакторов уже применены)' },
    @{ Name = 'Браузеры Playwright'; Path = "$env:LOCALAPPDATA\ms-playwright"; Safe = 'да'; How = 'удалить; npx playwright install скачает заново' },
    @{ Name = 'Кэш Puppeteer'; Path = "$home_\.cache\puppeteer"; Safe = 'да'; How = 'удалить папку' },
    @{ Name = 'Кэш Chrome'; Path = "$env:LOCALAPPDATA\Google\Chrome\User Data\Default\Cache"; Safe = 'да'; How = 'Chrome → Очистить историю → Изображения и файлы' },
    @{ Name = 'Кэш Code (VS Code)'; Path = "$env:APPDATA\Code\Cache"; Safe = 'да'; How = 'удалить папку при закрытом VS Code' },
    @{ Name = 'Кэш CachedData VS Code'; Path = "$env:APPDATA\Code\CachedData"; Safe = 'да'; How = 'удалить папку при закрытом VS Code' },
    @{ Name = 'Обновления Docker Desktop'; Path = "$env:LOCALAPPDATA\Temp\DockerDesktopUpdates"; Safe = 'да'; How = 'удалить папку' },
    @{ Name = 'Корзина (все диски)'; Path = $null; Safe = 'да'; How = 'очистить корзину' }
)
$caches = @()
foreach ($c in $cacheDefs) {
    if ($c.Path) {
        if (Test-Path -LiteralPath $c.Path) { $s = Get-TreeStats $c.Path; $gb = Format-Gb $s.Bytes; $files = $s.Files } else { $gb = 0; $files = 0 }
    } else {
        $rb = (New-Object -ComObject Shell.Application).NameSpace(0xA)
        $bytes = [long]0; foreach ($it in $rb.Items()) { $bytes += $it.Size }
        $gb = Format-Gb $bytes; $files = $rb.Items().Count
    }
    $caches += [pscustomobject]@{ Name = $c.Name; Path = $c.Path; GB = $gb; Files = $files; Safe = $c.Safe; How = $c.How }
}
$caches = $caches | Sort-Object GB -Descending

# ---------------------------------------------------------------- 6. installed programs

Write-Host "[6/12] Установленные программы..." -ForegroundColor Cyan
$uninstKeys = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
$programs = Get-ItemProperty $uninstKeys -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -and -not $_.SystemComponent } | ForEach-Object {
    $date = $null
    if ($_.InstallDate -match '^\d{8}$') { $date = $_.InstallDate.Substring(0, 4) + '-' + $_.InstallDate.Substring(4, 2) + '-' + $_.InstallDate.Substring(6, 2) }
    [pscustomobject]@{ Name = $_.DisplayName; Version = $_.DisplayVersion; Publisher = $_.Publisher; Installed = $date
        MB = $(if ($_.EstimatedSize) { [math]::Round($_.EstimatedSize / 1024) } else { $null }); Location = $_.InstallLocation; Uninstall = $_.UninstallString }
} | Sort-Object Name -Unique | Sort-Object MB -Descending
$storeApps = Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { -not $_.IsFramework -and $_.SignatureKind -eq 'Store' } | ForEach-Object {
    [pscustomobject]@{ Name = $_.Name; Version = $_.Version; Location = $_.InstallLocation }
}

# ---------------------------------------------------------------- 7. startup

Write-Host "[7/12] Автозагрузка..." -ForegroundColor Cyan
$startup = @()
foreach ($k in 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run', 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run', 'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run') {
    if (Test-Path $k) {
        $props = Get-ItemProperty $k
        foreach ($p in $props.PSObject.Properties) {
            if ($p.Name -like 'PS*') { continue }
            $startup += [pscustomobject]@{ Name = $p.Name; Command = $p.Value; Source = $k.Replace(':\Software\Microsoft\Windows\CurrentVersion\Run', '').Replace(':\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run', ' (32-bit)') }
        }
    }
}
foreach ($f in "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Startup", "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Startup") {
    foreach ($i in (Get-ChildItem -LiteralPath $f -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne 'desktop.ini' })) {
        $startup += [pscustomobject]@{ Name = $i.BaseName; Command = $i.FullName; Source = 'Папка автозагрузки' }
    }
}

# ---------------------------------------------------------------- 8. games and engines

Write-Host "[8/12] Игры и движки..." -ForegroundColor Cyan
$steamLibs = @()
$vdf = 'C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf'
if (Test-Path $vdf) {
    foreach ($m in [regex]::Matches((Get-Content $vdf -Raw), '"path"\s+"([^"]+)"')) {
        $lib = $m.Groups[1].Value.Replace('\\', '\')
        $common = Join-Path $lib 'steamapps\common'
        $games = @(); foreach ($g in (Get-ChildStats $common)) { $games += $g }
        $steamLibs += [pscustomobject]@{ Library = $lib; Games = $games; GB = [math]::Round(($games | Measure-Object GB -Sum).Sum, 1) }
    }
}
$epic = @()
foreach ($m in (Get-ChildItem 'C:\ProgramData\Epic\EpicGamesLauncher\Data\Manifests' -Filter '*.item' -ErrorAction SilentlyContinue)) {
    try { $j = Get-Content $m.FullName -Raw | ConvertFrom-Json; $epic += [pscustomobject]@{ Name = $j.DisplayName; Location = $j.InstallLocation; GB = $(if ($j.InstallSize) { Format-Gb $j.InstallSize } else { $null }) } } catch { }
}
$epicDirs = @(); foreach ($r in (Get-ChildStats 'C:\Program Files\Epic Games')) { $epicDirs += $r }
$unityEditors = @(); foreach ($r in (Get-ChildStats 'C:\Program Files\Unity\Hub\Editor')) { $unityEditors += $r }
$engines = @()
foreach ($p in @("$home_\AppData\Roaming\Godot", "$home_\AppData\Local\Godot", 'F:\PROJECTS\godot-hd\engine\bin', 'C:\Program Files\Blender Foundation', 'C:\Program Files\Cascadeur', 'C:\VulkanSDK', 'C:\AMD')) {
    if (Test-Path -LiteralPath $p) { $s = Get-TreeStats $p; $engines += [pscustomobject]@{ Path = $p; GB = Format-Gb $s.Bytes; Files = $s.Files } }
}

# ---------------------------------------------------------------- 9. Docker / WSL

Write-Host "[9/12] Docker и WSL..." -ForegroundColor Cyan
$wsl = @()
try { $raw = & wsl.exe -l -v 2>$null; if ($raw) { $wsl = ($raw -join "`n") -replace "`0", '' -split "`n" | Where-Object { $_.Trim() } } } catch { }
$vhdx = @()
foreach ($root in @("$env:LOCALAPPDATA\Docker", "$env:LOCALAPPDATA\Packages", 'D:\', 'F:\')) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    $depth = $(if ($root -match '^[A-Z]:\\$') { 3 } else { 6 })
    foreach ($f in (Get-ChildItem -LiteralPath $root -Recurse -Depth $depth -Filter '*.vhdx' -File -Force -ErrorAction SilentlyContinue)) {
        $vhdx += [pscustomobject]@{ Path = $f.FullName; GB = Format-Gb $f.Length 1; Modified = $f.LastWriteTime.ToString('yyyy-MM-dd') }
    }
}
$dockerSettings = $null
$ds = "$env:APPDATA\Docker\settings-store.json"
if (Test-Path $ds) { try { $j = Get-Content $ds -Raw | ConvertFrom-Json; $dockerSettings = [pscustomobject]@{ DataFolder = $j.DataFolder; DiskSizeMiB = $j.DiskSizeMiB; WslEngine = $j.WslEngineEnabled } } catch { } }

# ---------------------------------------------------------------- 10. Downloads

Write-Host "[10/12] Загрузки..." -ForegroundColor Cyan
$dl = Join-Path $home_ 'Downloads'
$dlFiles = Get-ChildItem -LiteralPath $dl -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne 'desktop.ini' }
$dlByCat = $dlFiles | Group-Object { Get-DownloadCategory $_ } | ForEach-Object {
    [pscustomobject]@{ Category = $_.Name; Count = $_.Count; GB = Format-Gb (($_.Group | Measure-Object Length -Sum).Sum) }
} | Sort-Object GB -Descending
$dlInstallers = $dlFiles | Where-Object { (Get-DownloadCategory $_) -eq 'Установщики' } | Sort-Object Length -Descending | ForEach-Object {
    [pscustomobject]@{ Name = $_.Name; MB = Format-Mb $_.Length; Modified = $_.LastWriteTime.ToString('yyyy-MM-dd') }
}
$dlDirs = @(); foreach ($r in (Get-ChildStats $dl)) { $dlDirs += [pscustomobject]@{ Name = $r.Name; GB = $r.GB; Files = $r.Files; Kind = (Test-ProjectType $r.Path); Modified = $r.Modified } }
$dlDupes = $dlFiles | Where-Object { $_.BaseName -match ' \(\d+\)$' } | ForEach-Object {
    $orig = Join-Path $dl (($_.BaseName -replace ' \(\d+\)$', '') + $_.Extension)
    [pscustomobject]@{ Copy = $_.Name; MB = Format-Mb $_.Length; OriginalExists = (Test-Path -LiteralPath $orig); SameSize = $(if (Test-Path -LiteralPath $orig) { (Get-Item -LiteralPath $orig).Length -eq $_.Length } else { $false }) }
}
$dlOld = $dlFiles | Where-Object { $_.LastWriteTime -lt $script:Started.AddDays(-90) }
$dlOldStats = [pscustomobject]@{ Count = @($dlOld).Count; GB = Format-Gb (($dlOld | Measure-Object Length -Sum).Sum) }

# ---------------------------------------------------------------- 11. Desktop, Documents, projects

Write-Host "[11/12] Рабочий стол, Документы, проекты..." -ForegroundColor Cyan
function Get-FolderInventory([string]$Path) {
    $rows = @()
    foreach ($i in (Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne 'desktop.ini' })) {
        if ($i.PSIsContainer) {
            if (($i.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
            $s = Get-TreeStats $i.FullName
            $rows += [pscustomobject]@{ Name = $i.Name; Type = 'папка'; Kind = (Test-ProjectType $i.FullName); MB = Format-Mb $s.Bytes; Files = $s.Files; Modified = $i.LastWriteTime.ToString('yyyy-MM-dd') }
        } else {
            $rows += [pscustomobject]@{ Name = $i.Name; Type = $i.Extension.TrimStart('.').ToLowerInvariant(); Kind = ''; MB = Format-Mb $i.Length; Files = 1; Modified = $i.LastWriteTime.ToString('yyyy-MM-dd') }
        }
    }
    return ($rows | Sort-Object MB -Descending)
}
$desktopInv = Get-FolderInventory (Join-Path $home_ 'Desktop')
$documentsInv = Get-FolderInventory (Join-Path $home_ 'Documents')

$roots = @((Join-Path $home_ 'Desktop'), (Join-Path $home_ 'Documents'), (Join-Path $home_ 'Downloads'), $home_, 'C:\', 'D:\', 'F:\') + $ProjectRoots
foreach ($v in $volumes) { $r = "$($v.Drive)\"; if ($roots -notcontains $r) { $roots += $r } }
$projects = @()
$seen = @{}
function Add-ProjectsFrom([string]$Root, [int]$Depth) {
    if (-not (Test-Path -LiteralPath $Root)) { return }
    $cands = @()
    $level = @((Get-Item -LiteralPath $Root -Force -ErrorAction SilentlyContinue))
    for ($i = 0; $i -lt $Depth; $i++) {
        $next = @()
        foreach ($d in $level) {
            foreach ($c in (Get-ChildItem -LiteralPath $d.FullName -Directory -Force -ErrorAction SilentlyContinue)) {
                if ($c.Name -in @('Windows', 'Program Files', 'Program Files (x86)', 'ProgramData', 'AppData', '$Recycle.Bin', '$RECYCLE.BIN', 'System Volume Information', 'Recovery', 'PerfLogs', 'inetpub', 'node_modules', '.git', 'Library', 'target', '.godot', 'steamapps', 'Users', 'PC-Audit')) { continue }
                if ($c.Name -like '.*' -and $c.FullName -like "$home_\*") { continue }
                if (($c.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
                $cands += $c; $next += $c
            }
        }
        $level = $next
    }
    foreach ($c in $cands) {
        $kind = Test-ProjectType $c.FullName
        if (-not $kind) { continue }
        # Skip nested projects whose parent is already a project (keep the outermost).
        $parent = Split-Path $c.FullName -Parent
        if ($seen.ContainsKey($parent.ToLowerInvariant())) { continue }
        if ($seen.ContainsKey($c.FullName.ToLowerInvariant())) { continue }
        $seen[$c.FullName.ToLowerInvariant()] = $true
        $s = Get-TreeStats $c.FullName
        $build = Get-BuildDirBytes $c.FullName
        $script:projects += [pscustomobject]@{ Path = $c.FullName; Kind = $kind; GB = Format-Gb $s.Bytes; BuildGB = Format-Gb $build; Files = $s.Files; Modified = $c.LastWriteTime.ToString('yyyy-MM-dd') }
    }
}
foreach ($r in ($roots | Select-Object -Unique)) {
    $depth = $(if ($r -match '^[A-Z]:\\$') { 2 } else { 2 })
    Add-ProjectsFrom $r $depth
}
$projects = $projects | Sort-Object GB -Descending

# ---------------------------------------------------------------- 12. big files

$bigFiles = @()
if (-not $Quick) {
    Write-Host "[12/12] Большие файлы (> $BigFileMB МБ) на всех дисках, кроме Windows..." -ForegroundColor Cyan
    foreach ($v in $volumes) {
        $bigFiles += Find-BigFiles "$($v.Drive)\" ([long]$BigFileMB * 1MB) @('Windows', '$Recycle.Bin', '$RECYCLE.BIN', 'System Volume Information', 'WinSxS')
    }
    $bigFiles = $bigFiles | Sort-Object GB -Descending | Select-Object -First 80
} else { Write-Host "[12/12] Большие файлы: пропущено (-Quick)" -ForegroundColor DarkGray }

# ---------------------------------------------------------------- recommendations

$recs = New-Object System.Collections.Generic.List[string]
$cDrive = $volumes | Where-Object { $_.Drive -eq 'C:' }
if ($cDrive -and $cDrive.FreePct -lt 15) { $recs.Add("На C: свободно всего $($cDrive.FreeGB) ГБ ($($cDrive.FreePct) %). Windows нуждается хотя бы в 30-50 ГБ запаса под обновления и файлы подкачки.") }
$cleanable = ($caches | Where-Object { $_.Safe -like 'да*' } | Measure-Object GB -Sum).Sum
if ($cleanable -gt 1) { $recs.Add("Кэши, которые можно удалить без потерь: около $([math]::Round($cleanable,1)) ГБ (таблица «Кэши»). Самые крупные: " + (($caches | Where-Object { $_.Safe -like 'да*' -and $_.GB -ge 1 } | ForEach-Object { "$($_.Name) $($_.GB) ГБ" }) -join ', ') + '.') }
if ($vhdx.Count -gt 1) { $recs.Add('Найдено несколько образов .vhdx (Docker/WSL). Если один из них — старая копия, её можно удалить; активный путь показывает Docker Desktop → Settings → Resources → Disk image location.') }
if ($sysFiles | Where-Object { $_.File -like '*hiberfil.sys' -and $_.GB -gt 4 }) { $recs.Add("hiberfil.sys занимает $(($sysFiles | Where-Object { $_.File -like '*hiberfil.sys' }).GB) ГБ. Если гибернация не нужна: powercfg /h off (от администратора).") }
if ($dlInstallers.Count -gt 0) { $recs.Add("В «Загрузках» $($dlInstallers.Count) установщиков на $([math]::Round(($dlFiles | Where-Object { (Get-DownloadCategory $_) -eq 'Установщики' } | Measure-Object Length -Sum).Sum/1GB,1)) ГБ — их место в папке Software\Installers, а то, что уже установлено, можно удалить.") }
if ($dlDupes.Count -gt 0) { $recs.Add("В «Загрузках» $($dlDupes.Count) файлов-копий вида «имя (1).ext»; у $(($dlDupes | Where-Object { $_.SameSize }).Count) из них оригинал того же размера лежит рядом.") }
if ($dlOldStats.Count -gt 0) { $recs.Add("$($dlOldStats.Count) файлов в «Загрузках» старше 90 дней ($($dlOldStats.GB) ГБ).") }
$buildTotal = ($projects | Measure-Object BuildGB -Sum).Sum
if ($buildTotal -gt 5) { $recs.Add("Папки сборки и зависимостей внутри проектов (node_modules, target, Library, .godot) занимают около $([math]::Round($buildTotal,1)) ГБ; у неактивных проектов их можно удалить — они восстанавливаются командой сборки.") }
if ($unityEditors.Count -gt 1) { $recs.Add("Установлено $($unityEditors.Count) редакторов Unity ($([math]::Round(($unityEditors | Measure-Object GB -Sum).Sum,1)) ГБ). Обычно нужен один — лишние удаляются через Unity Hub.") }
$steamC = $steamLibs | Where-Object { $_.Library -like 'C:\*' }
if ($steamC -and $steamC.GB -gt 5) { $recs.Add("Игры Steam на C: занимают $($steamC.GB) ГБ; их можно перенести на другой диск через Steam → Свойства игры → Локальные файлы → Переместить.") }
$fort = $epicDirs | Where-Object { $_.Name -eq 'Fortnite' }
if ($fort -and $fort.GB -gt 10) { $recs.Add("Fortnite на C: занимает $($fort.GB) ГБ; Epic переносит игры только переустановкой в новую папку (Epic Games Launcher → Библиотека → Удалить → Установить на другой диск).") }

# ---------------------------------------------------------------- write JSON

$data = [ordered]@{
    System = $system; PhysicalDisks = $physical; Volumes = $volumes; SystemFiles = $sysFiles; DriveTop = $driveTop
    Profile = $profileRows; AppData = $appData; Caches = $caches; Programs = $programs; StoreApps = $storeApps; Startup = $startup
    Steam = $steamLibs; Epic = $epic; EpicDirs = $epicDirs; UnityEditors = $unityEditors; Engines = $engines
    Wsl = $wsl; Vhdx = $vhdx; DockerSettings = $dockerSettings
    Downloads = [ordered]@{ ByCategory = $dlByCat; Installers = $dlInstallers; Folders = $dlDirs; Duplicates = $dlDupes; Old = $dlOldStats }
    Desktop = $desktopInv; Documents = $documentsInv; Projects = $projects; BigFiles = $bigFiles; Recommendations = $recs
    Duration = [math]::Round(((Get-Date) - $script:Started).TotalMinutes, 1)
}
$jsonPath = Join-Path $OutDir "PC-Audit-$stamp.json"
$data | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $jsonPath -Encoding UTF8

# ---------------------------------------------------------------- write Markdown

function Md-Table($rows, [string[]]$cols, [string[]]$headers) {
    if (-not $rows -or @($rows).Count -eq 0) { return '_нет данных_' }
    if (-not $headers) { $headers = $cols }
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine('| ' + ($headers -join ' | ') + ' |')
    [void]$sb.AppendLine('|' + (($cols | ForEach-Object { ' --- ' }) -join '|') + '|')
    foreach ($r in $rows) {
        $cells = foreach ($c in $cols) { $v = $r.$c; if ($null -eq $v) { '' } else { ("$v" -replace '\|', '\|') -replace "`n", ' ' } }
        [void]$sb.AppendLine('| ' + ($cells -join ' | ') + ' |')
    }
    return $sb.ToString()
}

$md = New-Object System.Text.StringBuilder
$w = { param($s) [void]$md.AppendLine($s) }
& $w "# Аудит компьютера — $($system.Computer), $($script:Started.ToString('yyyy-MM-dd HH:mm'))"
& $w ''
& $w 'Отчёт только читает диск и ничего не меняет. Сырые данные — в JSON рядом с этим файлом.'
& $w ''
& $w '## 1. Система'
& $w ''
foreach ($k in $system.Keys) { & $w "- **$k**: $($system[$k])" }
& $w ''
& $w '## 2. Диски'
& $w ''
& $w (Md-Table $physical @('Disk', 'Model', 'Type', 'GB', 'Health') @('Диск', 'Модель', 'Тип', 'ГБ', 'Состояние'))
& $w (Md-Table $volumes @('Drive', 'Label', 'FS', 'Disk', 'TotalGB', 'UsedGB', 'FreeGB', 'FreePct') @('Том', 'Метка', 'ФС', 'Диск', 'Всего ГБ', 'Занято ГБ', 'Свободно ГБ', 'Свободно %'))
& $w (Md-Table $sysFiles @('File', 'GB') @('Системный файл', 'ГБ'))
& $w '## 3. Что занимает место на каждом диске (папки верхнего уровня)'
& $w ''
foreach ($k in $driveTop.Keys) {
    & $w "### $k"
    & $w ''
    & $w (Md-Table $driveTop[$k] @('Name', 'GB', 'Files', 'Modified') @('Папка', 'ГБ', 'Файлов', 'Изменена'))
}
& $w "## 4. Профиль пользователя ($home_)"
& $w ''
& $w (Md-Table $profileRows @('Name', 'GB', 'Files', 'Kind', 'Modified') @('Папка', 'ГБ', 'Файлов', 'Тип', 'Изменена'))
& $w "## 5. AppData и скрытые папки (топ $TopN)"
& $w ''
& $w (Md-Table ($appData | Select-Object -First $TopN) @('Path', 'GB', 'Files') @('Путь', 'ГБ', 'Файлов'))
& $w '### Кэши: что можно чистить'
& $w ''
& $w (Md-Table $caches @('Name', 'GB', 'Files', 'Safe', 'How') @('Кэш', 'ГБ', 'Файлов', 'Можно удалять', 'Как'))
& $w "## 6. Установленные программы ($(@($programs).Count))"
& $w ''
& $w (Md-Table $programs @('Name', 'Version', 'Publisher', 'Installed', 'MB', 'Location') @('Программа', 'Версия', 'Издатель', 'Установлена', 'МБ', 'Папка'))
& $w "### Приложения из Microsoft Store ($(@($storeApps).Count))"
& $w ''
& $w (Md-Table $storeApps @('Name', 'Version') @('Пакет', 'Версия'))
& $w '## 7. Автозагрузка'
& $w ''
& $w (Md-Table $startup @('Name', 'Command', 'Source') @('Имя', 'Команда', 'Откуда'))
& $w '## 8. Игры и движки'
& $w ''
foreach ($lib in $steamLibs) {
    & $w "### Steam: $($lib.Library) — $($lib.GB) ГБ"
    & $w ''
    & $w (Md-Table $lib.Games @('Name', 'GB', 'Files', 'Modified') @('Игра', 'ГБ', 'Файлов', 'Изменена'))
}
& $w '### Epic Games'
& $w ''
& $w (Md-Table $epic @('Name', 'Location', 'GB') @('Приложение', 'Папка', 'ГБ (по манифесту)'))
& $w (Md-Table $epicDirs @('Name', 'GB', 'Files') @('Папка в Program Files\Epic Games', 'ГБ', 'Файлов'))
& $w '### Редакторы Unity'
& $w ''
& $w (Md-Table $unityEditors @('Name', 'GB', 'Files') @('Версия', 'ГБ', 'Файлов'))
& $w '### Прочие движки и SDK'
& $w ''
& $w (Md-Table $engines @('Path', 'GB', 'Files') @('Путь', 'ГБ', 'Файлов'))
& $w '## 9. Docker и WSL'
& $w ''
foreach ($l in $wsl) { & $w "    $l" }
& $w ''
if ($dockerSettings) { & $w "Настройки Docker: DataFolder=«$($dockerSettings.DataFolder)», DiskSizeMiB=$($dockerSettings.DiskSizeMiB), WslEngine=$($dockerSettings.WslEngine)"; & $w '' }
& $w (Md-Table $vhdx @('Path', 'GB', 'Modified') @('Образ .vhdx', 'ГБ', 'Изменён'))
& $w "## 10. Загрузки ($(@($dlFiles).Count) файлов, $(Format-Gb (($dlFiles | Measure-Object Length -Sum).Sum)) ГБ; папок: $(@($dlDirs).Count))"
& $w ''
& $w (Md-Table $dlByCat @('Category', 'Count', 'GB') @('Категория', 'Файлов', 'ГБ'))
& $w '### Установщики в «Загрузках»'
& $w ''
& $w (Md-Table $dlInstallers @('Name', 'MB', 'Modified') @('Файл', 'МБ', 'Дата'))
& $w '### Папки в «Загрузках»'
& $w ''
& $w (Md-Table $dlDirs @('Name', 'GB', 'Files', 'Kind', 'Modified') @('Папка', 'ГБ', 'Файлов', 'Тип', 'Изменена'))
& $w "### Копии «имя (N)» ($(@($dlDupes).Count))"
& $w ''
& $w (Md-Table $dlDupes @('Copy', 'MB', 'OriginalExists', 'SameSize') @('Копия', 'МБ', 'Оригинал рядом', 'Тот же размер'))
& $w "Старше 90 дней: $($dlOldStats.Count) файлов, $($dlOldStats.GB) ГБ."
& $w ''
& $w '## 11. Рабочий стол'
& $w ''
& $w (Md-Table $desktopInv @('Name', 'Type', 'Kind', 'MB', 'Files', 'Modified') @('Элемент', 'Тип', 'Проект', 'МБ', 'Файлов', 'Изменён'))
& $w '## 12. Документы'
& $w ''
& $w (Md-Table $documentsInv @('Name', 'Type', 'Kind', 'MB', 'Files', 'Modified') @('Элемент', 'Тип', 'Проект', 'МБ', 'Файлов', 'Изменён'))
& $w "## 13. Проекты, найденные на дисках ($(@($projects).Count))"
& $w ''
& $w 'BuildGB — папки сборки и зависимостей внутри проекта (node_modules, target, Library, .godot …), которые восстанавливаются заново.'
& $w ''
& $w (Md-Table $projects @('Path', 'Kind', 'GB', 'BuildGB', 'Files', 'Modified') @('Путь', 'Тип', 'ГБ', 'Из них сборка ГБ', 'Файлов', 'Изменён'))
if (-not $Quick) {
    & $w "## 14. Большие файлы (> $BigFileMB МБ, без папки Windows)"
    & $w ''
    & $w (Md-Table $bigFiles @('Path', 'GB', 'Modified') @('Файл', 'ГБ', 'Изменён'))
}
& $w '## 15. Рекомендации'
& $w ''
$i = 1; foreach ($r in $recs) { & $w "$i. $r"; $i++ }
& $w ''
& $w "_Аудит занял $($data.Duration) мин._"

$mdPath = Join-Path $OutDir "PC-Audit-$stamp.md"
[System.IO.File]::WriteAllText($mdPath, $md.ToString(), (New-Object System.Text.UTF8Encoding $true))
Write-Host ''
Write-Host "Готово: $mdPath" -ForegroundColor Green
Write-Host "        $jsonPath" -ForegroundColor Green
