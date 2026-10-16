# Invoke-PcOrganize.ps1 - sorts the user's files into a fixed layout: software to
# <Root>\Software, projects to <Root>\PROJECTS, joint CodeWhale work to <Root>\AI\CodeWhale,
# backups/duplicates to <Root>\Archive, documents/pictures/videos into the user's folders.
#
# Without -Apply it only prints the plan and writes PC-Organize-Plan-<date>.csv/.md to
# <Desktop>\PC-Audit. With -Apply every move is written to PC-Organize-Journal-<date>.csv;
# -Undo <journal> moves everything back. The script never deletes anything: items that
# look superfluous are listed as "DELETE?" candidates for the user to decide.
#
# Windows PowerShell 5.1; saved as UTF-8 with BOM.
#
# Usage (dry run, everything):
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcOrganize.ps1 -Root F:\ -All
# Apply the same plan:
#   ... -Root F:\ -All -Apply
# Apply an edited plan (delete rows you do not want from the CSV first):
#   ... -PlanFile "$env:USERPROFILE\Desktop\PC-Audit\PC-Organize-Plan-<date>.csv" -Apply
# Undo:
#   ... -Undo "$env:USERPROFILE\Desktop\PC-Audit\PC-Organize-Journal-<date>.csv"
[CmdletBinding()]
param(
    [string]$Root = 'F:\',
    [string]$UnityProjectsDir = 'D:\MY_UNITY_PROJECTS',
    [string]$OutDir,
    [switch]$All,
    [switch]$SortDownloads,
    [switch]$SortDesktop,
    [switch]$SortDocuments,
    [switch]$MoveProjects,
    [switch]$MoveInstallers,
    [switch]$MovePortable,
    [switch]$MoveWorkFolders,
    [switch]$CleanRoots,
    [switch]$ArchiveDuplicates,
    [switch]$MoveRepo,
    [switch]$Apply,
    [string]$PlanFile,
    [string]$Undo,
    [ValidateSet('Rename', 'Skip')][string]$OnConflict = 'Rename'
)

$ErrorActionPreference = 'Stop'
$stamp = (Get-Date).ToString('yyyy-MM-dd_HH-mm')
$home_ = $env:USERPROFILE
if (-not $OutDir) { $OutDir = Join-Path ([Environment]::GetFolderPath('Desktop')) 'PC-Audit' }
if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
if ($All) { $SortDownloads = $SortDesktop = $SortDocuments = $MoveProjects = $MoveInstallers = $MovePortable = $MoveWorkFolders = $CleanRoots = $ArchiveDuplicates = $true }

# ---------------------------------------------------------------- layout

$Root = $Root.TrimEnd('\') + '\'
$L = [ordered]@{
    Software      = Join-Path $Root 'Software'
    Installers    = Join-Path $Root 'Software\Installers'
    Portable      = Join-Path $Root 'Software\Portable'
    Drivers       = Join-Path $Root 'Software\Drivers'
    UnrealAssets  = Join-Path $Root 'Software\Unreal-Assets'
    Work          = Join-Path $Root 'AI\CodeWhale'
    Projects      = Join-Path $Root 'PROJECTS'
    GodotSandbox  = Join-Path $Root 'PROJECTS\_godot-sandbox'
    Archive       = Join-Path $Root 'Archive'
    Backups       = Join-Path $Root 'Archive\Backups'
    DbDumps       = Join-Path $Root 'Archive\DB-dumps'
    OldProjects   = Join-Path $Root 'Archive\Old-Projects'
    Misc          = Join-Path $Root 'Archive\Misc'
    Art           = Join-Path $Root 'ART BASE'
    Artbooks      = Join-Path $Root 'ART BASE\Artbooks'
    ArtReference  = Join-Path $Root 'ART BASE\Reference'
    Drawings      = Join-Path $Root 'ART BASE\Drawings'
    Art3D         = Join-Path $Root 'ART BASE\3D'
    Unity         = $UnityProjectsDir
    Documents     = Join-Path $home_ 'Documents'
    WorkSalary    = Join-Path $home_ 'Documents\Work\Salary'
    WorkCs        = Join-Path $home_ 'Documents\Work\CS-System'
    WorkOther     = Join-Path $home_ 'Documents\Work\Other'
    Personal      = Join-Path $home_ 'Documents\Personal'
    Security      = Join-Path $home_ 'Documents\Personal\Security'
    GuidesGodot   = Join-Path $home_ 'Documents\Guides\Godot'
    GuidesUnity   = Join-Path $home_ 'Documents\Guides\Unity'
    GuidesAI      = Join-Path $home_ 'Documents\Guides\AI-agents'
    Books         = Join-Path $home_ 'Documents\Books'
    GuidesArt     = Join-Path $home_ 'Documents\Guides\Art'
    GuidesOther   = Join-Path $home_ 'Documents\Guides\Other'
    DocsUnsorted  = Join-Path $home_ 'Documents\Unsorted'
    Pictures      = Join-Path $home_ 'Pictures\Downloads'
    Gif           = Join-Path $home_ 'Pictures\GIF'
    Screenshots   = Join-Path $home_ 'Pictures\Screenshots'
    Videos        = Join-Path $home_ 'Videos\Downloads'
    Downloads     = Join-Path $home_ 'Downloads'
    Torrents      = Join-Path $home_ 'Downloads\Torrents'
    DlUnsorted    = Join-Path $home_ 'Downloads\Unsorted'
    Desktop       = Join-Path $home_ 'Desktop'
}

# ---------------------------------------------------------------- helpers

$script:Plan = New-Object System.Collections.Generic.List[object]

function Get-TreeBytes([string]$Path) {
    $bytes = [long]0
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

function Add-Plan([string]$Group, [string]$Action, [string]$Source, [string]$Destination, [string]$Note = '') {
    if (-not (Test-Path -LiteralPath $Source)) { return }
    $item = Get-Item -LiteralPath $Source -Force
    $type = $(if ($item.PSIsContainer) { 'dir' } else { 'file' })
    $bytes = $(if ($item.PSIsContainer) { Get-TreeBytes $Source } else { $item.Length })
    $script:Plan.Add([pscustomobject]@{ Group = $Group; Action = $Action; Type = $type; Source = $Source; Destination = $Destination; SizeMB = [math]::Round($bytes / 1MB, 1); Note = $Note })
}

function Add-Move([string]$Group, [string]$Source, [string]$DestDir, [string]$Note = '', [string]$NewName = '') {
    $name = $(if ($NewName) { $NewName } else { Split-Path $Source -Leaf })
    Add-Plan $Group 'MOVE' $Source (Join-Path $DestDir $name) $Note
}

function Add-Delete([string]$Group, [string]$Source, [string]$Note) { Add-Plan $Group 'DELETE?' $Source '' $Note }
function Add-Manual([string]$Group, [string]$Source, [string]$Note) { Add-Plan $Group 'MANUAL' $Source '' $Note }

function Test-ProjectKind([string]$Path) {
    $t = @()
    $has = { param($n) Test-Path -LiteralPath (Join-Path $Path $n) }
    # A download folder that only wraps the real project (zip + extracted folder of the same name).
    $subs = @(Get-ChildItem -LiteralPath $Path -Directory -Force -ErrorAction SilentlyContinue)
    if ($subs.Count -eq 1 -and $subs[0].Name -eq (Split-Path $Path -Leaf)) { $inner = Test-ProjectKind $subs[0].FullName; if ($inner) { return $inner } }
    if (& $has '.git') { $t += 'git' }
    if (& $has 'project.godot') { $t += 'Godot' }
    if ((& $has 'Assets') -and (& $has 'ProjectSettings')) { $t += 'Unity' }
    if (& $has 'Cargo.toml') { $t += 'Rust' }
    if (& $has 'package.json') { $t += 'Node' }
    if (& $has 'docker-compose.yml') { $t += 'Docker' }
    if ((& $has 'pyproject.toml') -or (& $has 'requirements.txt')) { $t += 'Python' }
    if (Get-ChildItem -LiteralPath $Path -Filter '*.uefnproject' -File -ErrorAction SilentlyContinue | Select-Object -First 1) { $t += 'UEFN' }
    return ($t -join '+')
}

# Installed-program lookup (for "already installed" notes on installers).
$installedNames = @()
try {
    $installedNames = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName } | ForEach-Object { $_.DisplayName }
    if (Get-AppxPackage -Name 'Claude*' -ErrorAction SilentlyContinue) { $installedNames += 'Claude (Microsoft Store)' }
} catch { }

$installerMap = @(
    @{ Rx = 'nvidia|^\d{3}\.\d{2}-desktop'; Cat = 'Drivers'; Installed = 'NVIDIA Graphics Driver' },
    @{ Rx = 'amd-software|adrenalin|chipset'; Cat = 'Drivers'; Installed = 'AMD Software' },
    @{ Rx = 'veikk'; Cat = 'Drivers'; Installed = 'Veikk' },
    @{ Rx = 'antigravity'; Cat = 'Development'; Installed = 'Antigravity' },
    @{ Rx = '^fork-'; Cat = 'Development'; Installed = '^Fork' },
    @{ Rx = 'geany'; Cat = 'Development'; Installed = 'Geany' },
    @{ Rx = '^python-'; Cat = 'Development'; Installed = '^Python 3[\d.]+ \(64-bit\)' },
    @{ Rx = 'rustup'; Cat = 'Development'; Installed = 'Rustup' },
    @{ Rx = 'unityhub'; Cat = 'Development'; Installed = 'Unity Hub' },
    @{ Rx = 'docker desktop'; Cat = 'Development'; Installed = 'Docker Desktop' },
    @{ Rx = 'vulkansdk'; Cat = 'Development'; Installed = 'Vulkan SDK' },
    @{ Rx = 'tempora'; Cat = 'Development'; Installed = '^Tempora' },
    @{ Rx = 'claude setup'; Cat = 'AI-assistants'; Installed = 'Claude' },
    @{ Rx = '^kimi'; Cat = 'AI-assistants'; Installed = '^Kimi' },
    @{ Rx = '^qwen'; Cat = 'AI-assistants'; Installed = '^Qwen' },
    @{ Rx = '^csp_'; Cat = 'Graphics-3D'; Installed = 'CLIP STUDIO PAINT' },
    @{ Rx = 'cascadeur'; Cat = 'Graphics-3D'; Installed = 'Cascadeur' },
    @{ Rx = 'blender'; Cat = 'Graphics-3D'; Installed = '^Blender' },
    @{ Rx = '^tsetup'; Cat = 'Communication-Browsers'; Installed = 'Telegram Desktop' },
    @{ Rx = 'chromesetup'; Cat = 'Communication-Browsers'; Installed = 'Google Chrome' },
    @{ Rx = 'discord'; Cat = 'Communication-Browsers'; Installed = 'Discord' },
    @{ Rx = 'audacity'; Cat = 'Audio-Video'; Installed = 'Audacity' },
    @{ Rx = 'vlc'; Cat = 'Audio-Video'; Installed = 'VLC' },
    @{ Rx = 'bepinex|nitrogenia'; Cat = 'Games-Mods'; Installed = '' }
)

function Resolve-Installer([string]$Name) {
    foreach ($m in $installerMap) {
        if ($Name -imatch $m.Rx) {
            $note = ''
            if ($m.Installed) {
                $hit = $installedNames | Where-Object { $_ -imatch $m.Installed } | Select-Object -First 1
                if ($hit) { $note = "уже установлено: $hit — установщик можно удалить" }
            }
            return [pscustomobject]@{ Cat = $m.Cat; Note = $note }
        }
    }
    return [pscustomobject]@{ Cat = 'Other'; Note = '' }
}

# Document routing by keywords (ordered; first match wins).
$docRules = @(
    @{ Rx = 'backup-codes|recovery-codes|password generator'; Dest = $L.Security; Note = 'коды доступа — лучше хранить в менеджере паролей' },
    @{ Rx = 'cs system|support ticket|crystall|crystal|chlite|rbac|regulations it|тз_система|инхаус|kanboard|десктопное|анализ данных|оптимизация node|race condition|парсера и валидатора|графической аналитики|отказоустойчивости|безопасность сервера|система заявок|превью —|тз бд кс|qa-аналитика|материалы для веб|обучение веб|скрипта для анализа|deploy'; Dest = $L.WorkCs },
    @{ Rx = 'salary|сводная|export_operators|export_qa|operator form|qa form|mastertable|employees cards|^book \d|^info\.csv|salarydivider|salary-divider|salaryvalidator|salary-validator|гпх'; Dest = $L.WorkSalary },
    @{ Rx = 'kommo|leads\.txt|upwork|^it 20\d\d'; Dest = $L.WorkOther },
    @{ Rx = '\.epub$|\.fb2'; Dest = $L.Books },
    @{ Rx = 'ticket|visa|receipt|itinerary|invoice|resume|hotel|выписка|повест|военн|уведомлени|документ-20\d\d'; Dest = $L.Personal },
    @{ Rx = 'unity'; Dest = $L.GuidesUnity },
    @{ Rx = 'godot|playmaker|sky creator|аддон|gasp|feel|auto collision|fluid|воды|d3d12|mesa|реалистичной'; Dest = $L.GuidesGodot },
    @{ Rx = 'ии-агент|codewhale|handoff|vibe|local_agent'; Dest = $L.GuidesAI },
    @{ Rx = 'clip studio|anatomy|artbook|art of'; Dest = $L.GuidesArt },
    @{ Rx = 'технические требования|водонапорной|шаблон'; Dest = $L.GuidesOther }
)

function Resolve-Document([string]$Name) {
    foreach ($r in $docRules) { if ($Name -imatch $r.Rx) { return [pscustomobject]@{ Dest = $r.Dest; Note = $(if ($r.Note) { $r.Note } else { '' }) } } }
    return [pscustomobject]@{ Dest = $L.DocsUnsorted; Note = 'категория не распознана' }
}

# Generic file router used for Downloads, Desktop files, Telegram Desktop.
function Route-File([string]$Group, [System.IO.FileInfo]$f) {
    $n = $f.Name; $ext = $f.Extension.ToLowerInvariant()
    if ($n -eq 'desktop.ini') { return }
    switch -Regex ($ext) {
        '^\.(exe|msi|msix|appx)$' {
            if ($n -imatch '^rclone') { Add-Move $Group $f.FullName (Join-Path $L.Portable 'rclone') 'портативная утилита'; return }
            if (-not $MoveInstallers) { return }
            $r = Resolve-Installer $n
            Add-Move $Group $f.FullName (Join-Path $L.Installers $r.Cat) $r.Note; return
        }
        '^\.(zip|rar|7z|tar|gz|tgz|xz)$' {
            if ($n -imatch '\.sql\.gz$|db_backup|database') { Add-Move $Group $f.FullName $L.DbDumps; return }
            if ($n -imatch '\.(jpg|jpeg|png)\.zip$|photo') { Add-Move $Group $f.FullName $L.Pictures 'архив с фото'; return }
            if ($n -imatch 'godot_v') { Add-Move $Group $f.FullName (Join-Path $L.Portable 'Godot-4.7.2') 'портативный Godot'; return }
            if ($n -imatch 'dbeaver') { Add-Move $Group $f.FullName (Join-Path $L.Portable 'DBeaver'); return }
            if ($n -imatch '^chlite') { Add-Move $Group $f.FullName (Join-Path $L.Portable 'chlite'); return }
            if ($n -imatch 'bepinex|nitrogenia') { Add-Move $Group $f.FullName (Join-Path $L.Installers 'Games-Mods') 'моды STRAFTAT'; return }
            if ($n -imatch 'sky creator|fluid') { Add-Move $Group $f.FullName $L.UnrealAssets; return }
            if ($n -imatch 'codewhale|local_agent_project') { Add-Move $Group $f.FullName (Join-Path $L.Work 'releases'); return }
            Add-Move $Group $f.FullName $L.Backups 'архив проекта/данных'; return
        }
        '^\.(sql|db|sqlite)$' { Add-Move $Group $f.FullName $L.DbDumps; return }
        '^\.(docx|doc|odt|rtf|txt|md|pdf|xlsx|xls|csv|pptx|html|htm|epub|fb2)$' {
            if ($n -imatch '^Human_Anatomy') { Add-Move $Group $f.FullName $L.ArtReference; return }
            $r = Resolve-Document $n
            Add-Move $Group $f.FullName $r.Dest $r.Note; return
        }
        '^\.gif$' { Add-Move $Group $f.FullName $L.Gif; return }
        '^\.(png|jpg|jpeg|webp|bmp|svg|psd)$' {
            if ($n -imatch '^screenshot') { Add-Move $Group $f.FullName $L.Screenshots } else { Add-Move $Group $f.FullName $L.Pictures }
            return
        }
        '^\.clip$' { Add-Move $Group $f.FullName $L.Drawings; return }
        '^\.(blend|fbx|glb|gltf|obj)$' { Add-Move $Group $f.FullName $L.Art3D; return }
        '^\.(mp4|mov|mkv|avi|webm)$' { Add-Move $Group $f.FullName $L.Videos; return }
        '^\.torrent$' { Add-Move $Group $f.FullName $L.Torrents; return }
        '^\.(lnk|url)$' { return }
        default { Add-Move $Group $f.FullName $L.DlUnsorted 'не распознано'; return }
    }
}

# ---------------------------------------------------------------- plan: Downloads

if ($SortDownloads -and -not $PlanFile -and -not $Undo) {
    $dl = $L.Downloads
    foreach ($f in (Get-ChildItem -LiteralPath $dl -File -Force)) { Route-File 'Downloads' $f }
    foreach ($d in (Get-ChildItem -LiteralPath $dl -Directory -Force)) {
        $n = $d.Name
        if ($n -in @('Torrents', 'Unsorted')) { continue }
        switch -Regex ($n) {
            '^Artbook' { Add-Move 'Downloads' $d.FullName $L.Artbooks; continue }
            '^chlite' { if ($MovePortable) { Add-Move 'Downloads' $d.FullName (Join-Path $L.Portable 'chlite') }; continue }
            '^CodeWhale-Desktop.*portable' { if ($MoveWorkFolders) { Add-Move 'Downloads' $d.FullName (Join-Path $L.Work 'portable') 'закрыть запущенный portable CodeWhale Desktop перед переносом' }; continue }
            '^dbeaver' { if ($MovePortable) { Add-Move 'Downloads' $d.FullName (Join-Path $L.Portable 'DBeaver') }; continue }
            'Fluid Flux|FluidNinja' { Add-Move 'Downloads' $d.FullName $L.UnrealAssets 'плагины Unreal'; continue }
            '^LOCAL_AGENT_PROJECT' {
                $inner = Join-Path $d.FullName $n
                if ($MoveRepo) {
                    Add-Move 'Repo' $inner $L.Work 'репозиторий CodeWhale: закрыть приложение и сессию Claude, затем запускать claude из новой папки' 'LOCAL_AGENT_PROJECT'
                    Add-Delete 'Repo' $d.FullName 'после переноса репозитория останутся только журналы сборки (npm-ci.log, tauri-dev.log …)'
                } else { Add-Manual 'Repo' $inner "репозиторий CodeWhale — переносится в $($L.Work)\LOCAL_AGENT_PROJECT ключом -MoveRepo (последним шагом, при закрытом приложении и сессии)" }
                continue
            }
            '^Task_Manager_Project' { if ($ArchiveDuplicates) { Add-Move 'Downloads' $d.FullName $L.OldProjects 'копия проекта, оригиналы в PROJECTS' }; continue }
            '^Telegram Desktop$' {
                foreach ($f in (Get-ChildItem -LiteralPath $d.FullName -File -Force)) { Route-File 'Telegram' $f }
                foreach ($s in (Get-ChildItem -LiteralPath $d.FullName -Directory -Force)) {
                    if ((Test-ProjectKind $s.FullName) -or (Test-Path (Join-Path $s.FullName 'README.md'))) { if ($MoveProjects) { Add-Move 'Telegram' $s.FullName $L.Projects 'проект из Telegram' } }
                    else { Add-Move 'Telegram' $s.FullName $L.DlUnsorted }
                }
                continue
            }
            default {
                $kind = Test-ProjectKind $d.FullName
                if ($kind) { if ($MoveProjects) { Add-Move 'Downloads' $d.FullName $L.Projects "проект ($kind)" } }
                else { Add-Move 'Downloads' $d.FullName $L.DlUnsorted 'не распознано' }
            }
        }
    }
}

# ---------------------------------------------------------------- plan: Desktop

if ($SortDesktop -and -not $PlanFile -and -not $Undo) {
    $dt = $L.Desktop
    foreach ($f in (Get-ChildItem -LiteralPath $dt -File -Force)) {
        if ($f.Name -eq 'Modelfile') { if ($MoveWorkFolders) { Add-Move 'Desktop' $f.FullName (Join-Path $L.Work 'ollama') 'Modelfile для варианта модели Ollama' }; continue }
        if ($f.Extension -in @('.lnk', '.url')) { continue }
        Route-File 'Desktop' $f
    }
    foreach ($d in (Get-ChildItem -LiteralPath $dt -Directory -Force)) {
        switch -Regex ($d.Name) {
            '^PC-Audit$' { continue }
            '^ALL$' {
                foreach ($f in (Get-ChildItem -LiteralPath $d.FullName -File -Force)) {
                    if ($f.Extension -in @('.lnk', '.url')) { Add-Delete 'Desktop\ALL' $f.FullName 'ярлык на установленную программу/игру (есть в меню Пуск)'; continue }
                    Route-File 'Desktop\ALL' $f
                }
                foreach ($s in (Get-ChildItem -LiteralPath $d.FullName -Directory -Force)) {
                    $kind = Test-ProjectKind $s.FullName
                    $files = @(Get-ChildItem -LiteralPath $s.FullName -Recurse -File -Force -ErrorAction SilentlyContinue).Count
                    if ($files -eq 0) { Add-Delete 'Desktop\ALL' $s.FullName 'пустая папка'; continue }
                    if ($s.Name -imatch '^temp') { if ($ArchiveDuplicates) { Add-Move 'Desktop\ALL' $s.FullName $L.OldProjects 'временные скрипты' }; continue }
                    if ($kind -or $s.Name -in @('task_manager_local', 'ERP SYSTEM')) { if ($MoveProjects) { Add-Move 'Desktop\ALL' $s.FullName $L.Projects "проект ($kind)" }; continue }
                    if ($s.Name -imatch 'salar|^VD$') { Add-Move 'Desktop\ALL' $s.FullName $L.WorkSalary; continue }
                    if ($s.Name -imatch 'analysis') { Add-Move 'Desktop\ALL' $s.FullName $L.WorkOther; continue }
                    if ($s.Name -imatch 'education') { Add-Move 'Desktop\ALL' $s.FullName $L.WorkCs; continue }
                    Add-Move 'Desktop\ALL' $s.FullName $L.DocsUnsorted 'не распознано'
                }
                Add-Delete 'Desktop\ALL' $d.FullName 'после переноса содержимого папка останется пустой'
                continue
            }
            '^CodeWhale_Workspace$' { if ($MoveWorkFolders) { Add-Move 'Desktop' $d.FullName $L.OldProjects 'старый клон репозитория (один коммит); рабочая копия — в AI\CodeWhale; ярлык CodeWhale_Local.lnk и workspace в настройках приложения перенастроить' }; continue }
            '^Crystall_UI_preview$' { Add-Move 'Desktop' $d.FullName $L.WorkCs; continue }
            '^Godot_v.*\.exe$' {
                if ($MovePortable) {
                    foreach ($f in (Get-ChildItem -LiteralPath $d.FullName -File -Force)) { Add-Move 'Desktop' $f.FullName (Join-Path $L.Portable 'Godot-4.7.2') 'портативный Godot 4.7.2' }
                    Add-Delete 'Desktop' $d.FullName 'после переноса exe останется пустой'
                }
                continue
            }
            '^Octagon-Servers$' { Add-Manual 'Desktop' $d.FullName "приватный SSH-ключ и known_hosts; ssh_config ссылается на путь на рабочем столе. Переносить вручную в $home_\.ssh\octagon-servers и поправить пути в ssh_config"; continue }
            '^Salaries_app$' { Add-Move 'Desktop' $d.FullName $L.WorkSalary; continue }
            default {
                $kind = Test-ProjectKind $d.FullName
                if ($kind) { if ($MoveProjects) { Add-Move 'Desktop' $d.FullName $L.Projects "проект ($kind)" } }
                else { Add-Move 'Desktop' $d.FullName $L.DocsUnsorted 'не распознано' }
            }
        }
    }
}

# ---------------------------------------------------------------- plan: Documents and profile root

if ($SortDocuments -and -not $PlanFile -and -not $Undo) {
    foreach ($d in (Get-ChildItem -LiteralPath $L.Documents -Directory -Force)) {
        if (($d.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
        if ($d.Name -in @('Work', 'Personal', 'Guides', 'Unsorted', 'My Games', 'CAPCOM', 'JoyOfProgramming')) { continue }
        $kind = Test-ProjectKind $d.FullName
        $files = @(Get-ChildItem -LiteralPath $d.FullName -Recurse -File -Force -ErrorAction SilentlyContinue).Count
        if ($files -eq 0) { Add-Delete 'Documents' $d.FullName 'пустая папка'; continue }
        if ($kind -match 'Godot') { if ($MoveProjects) { Add-Move 'Documents' $d.FullName $L.GodotSandbox 'тестовый проект Godot' }; continue }
        if ($kind) { if ($MoveProjects) { Add-Move 'Documents' $d.FullName $L.Projects "проект ($kind)" }; continue }
    }
    foreach ($f in (Get-ChildItem -LiteralPath $L.Documents -File -Force | Where-Object { $_.Name -ne 'desktop.ini' })) { Route-File 'Documents' $f }
}

if ($MoveProjects -and -not $PlanFile -and -not $Undo) {
    foreach ($d in (Get-ChildItem -LiteralPath $home_ -Directory -Force)) {
        if ($d.Name -like '.*' -or $d.Name -in @('AppData', 'Desktop', 'Documents', 'Downloads', 'Pictures', 'Videos', 'Music', 'Contacts', 'Favorites', 'Links', 'Saved Games', 'Searches', 'OneDrive')) { continue }
        if (($d.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { continue }
        $kind = Test-ProjectKind $d.FullName
        $files = @(Get-ChildItem -LiteralPath $d.FullName -Recurse -File -Force -ErrorAction SilentlyContinue).Count
        if ($files -eq 0) { Add-Delete 'Profile' $d.FullName 'пустая папка'; continue }
        if ($kind -match 'Unity') { Add-Move 'Profile' $d.FullName $L.Unity 'проект Unity; после переноса добавить в Unity Hub заново (Add → папка)'; continue }
        if ($kind) { Add-Move 'Profile' $d.FullName $L.Projects "проект ($kind)"; continue }
    }
    $uefn = Join-Path $Root 'UEFN_PROJECTS'
    if (Test-Path -LiteralPath $uefn) { Add-Move 'Roots' $uefn $L.Projects 'проекты UEFN рядом с остальными' }
}

# ---------------------------------------------------------------- plan: drive roots

if ($CleanRoots -and -not $PlanFile -and -not $Undo) {
    if (Test-Path 'C:\Dev\phoenix') { Add-Move 'Roots' 'C:\Dev\phoenix' $L.Misc 'три текстовых файла' }
    if (Test-Path 'C:\Dev') { Add-Delete 'Roots' 'C:\Dev' 'после переноса phoenix останется пустой (файл null)' }
    foreach ($f in (Get-ChildItem 'C:\Anatomy' -File -ErrorAction SilentlyContinue)) { Add-Move 'Roots' $f.FullName $L.ArtReference }
    if (Test-Path 'C:\Anatomy') { Add-Delete 'Roots' 'C:\Anatomy' 'останется пустой' }
    foreach ($f in (Get-ChildItem 'C:\Drawings' -File -ErrorAction SilentlyContinue)) { Add-Move 'Roots' $f.FullName $L.Drawings }
    if (Test-Path 'C:\Drawings') { Add-Delete 'Roots' 'C:\Drawings' 'останется пустой' }
    if (Test-Path 'C:\inetpub') { Add-Delete 'Roots' 'C:\inetpub' 'пустая папка IIS' }
    if (Test-Path 'C:\AMD') { Add-Delete 'Roots' 'C:\AMD' 'распакованные установщики драйверов AMD (драйвер уже установлен)' }
    foreach ($f in (Get-ChildItem -LiteralPath $Root -File -Force -ErrorAction SilentlyContinue | Where-Object { $_.Extension -in @('.exe', '.msi') })) {
        $r = Resolve-Installer $f.Name
        Add-Move 'Roots' $f.FullName $(if ($r.Cat -eq 'Drivers') { $L.Drivers } else { Join-Path $L.Installers $r.Cat }) $r.Note
    }
    foreach ($n in 'tmpkh', 'UEFN') { $p = Join-Path $Root $n; if ((Test-Path -LiteralPath $p) -and @(Get-ChildItem -LiteralPath $p -Recurse -File -Force -ErrorAction SilentlyContinue).Count -eq 0) { Add-Delete 'Roots' $p 'пустая папка' } }
    $epic = Join-Path $Root 'EPIC'
    if (Test-Path -LiteralPath $epic) { Add-Delete 'Roots' $epic 'копия Epic Games Launcher; рабочий лаунчер стоит в C:\Program Files (x86)\Epic Games' }
    $dockerF = Join-Path $Root 'Docker\DockerDesktopWSL\disk\docker_data.vhdx'
    if (Test-Path -LiteralPath $dockerF) { Add-Delete 'Roots' (Join-Path $Root 'Docker') 'старый образ Docker (53 ГБ, 2026-10-04); активный — в %LOCALAPPDATA%\Docker\wsl. Сначала проверить Docker Desktop → Settings → Resources → Disk image location' }
}

# ---------------------------------------------------------------- plan: duplicates in PROJECTS

if ($ArchiveDuplicates -and -not $PlanFile -and -not $Undo) {
    foreach ($i in (Get-ChildItem -LiteralPath $L.Projects -Force -ErrorAction SilentlyContinue)) {
        if ($i.Name -imatch ' - Copy| - Backup|-dummy$' -or $i.Name -imatch '\.zip$') { Add-Move 'Duplicates' $i.FullName $L.OldProjects 'копия/бэкап, оригинал остаётся в PROJECTS' }
    }
}

# ---------------------------------------------------------------- undo

function Move-Entry([string]$Source, [string]$Destination, [string]$Type) {
    $destDir = Split-Path $Destination -Parent
    if (-not (Test-Path -LiteralPath $destDir)) { New-Item -ItemType Directory -Path $destDir -Force | Out-Null }
    $final = $Destination
    if (Test-Path -LiteralPath $final) {
        if ($OnConflict -eq 'Skip') { return [pscustomobject]@{ Status = 'SKIPPED'; Final = $final; Note = 'destination exists' } }
        $leaf = Split-Path $final -Leaf; $k = 2
        $base = [System.IO.Path]::GetFileNameWithoutExtension($leaf); $ext = [System.IO.Path]::GetExtension($leaf)
        if ($Type -eq 'dir') { $base = $leaf; $ext = '' }
        while (Test-Path -LiteralPath (Join-Path $destDir "$base ($k)$ext")) { $k++ }
        $final = Join-Path $destDir "$base ($k)$ext"
    }
    $sameVolume = ($Source.Substring(0, 2).ToUpperInvariant() -eq $final.Substring(0, 2).ToUpperInvariant())
    if ($Type -eq 'file' -or $sameVolume) {
        Move-Item -LiteralPath $Source -Destination $final -Force
        return [pscustomobject]@{ Status = 'OK'; Final = $final; Note = '' }
    }
    # Directory across volumes: robocopy /MOVE (handles long paths, copies attributes/timestamps).
    # Exit code is a bit mask: 1 copied, 2 extras, 4 mismatches, 8 some files failed, 16 fatal.
    $rc = & robocopy.exe $Source $final /E /MOVE /COPY:DAT /DCOPY:DAT /R:1 /W:1 /NFL /NDL /NJH /NJS /NP /XJ 2>&1
    $code = $LASTEXITCODE
    if ($code -ge 16 -and -not (Test-Path -LiteralPath $final)) { return [pscustomobject]@{ Status = 'FAILED'; Final = $final; Note = "robocopy exit $code" } }
    if (Test-Path -LiteralPath $Source) {
        $left = @(Get-ChildItem -LiteralPath $Source -Recurse -File -Force -ErrorAction SilentlyContinue).Count
        if ($left -gt 0) { return [pscustomobject]@{ Status = 'PARTIAL'; Final = $final; Note = "$left файлов не перенесено (заняты?), robocopy exit $code" } }
        Remove-Item -LiteralPath $Source -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($code -ge 8) { return [pscustomobject]@{ Status = 'PARTIAL'; Final = $final; Note = "robocopy exit $code" } }
    return [pscustomobject]@{ Status = 'OK'; Final = $final; Note = '' }
}

if ($Undo) {
    if (-not (Test-Path -LiteralPath $Undo)) { throw "Журнал не найден: $Undo" }
    $rows = @(Import-Csv -LiteralPath $Undo -Encoding UTF8 | Where-Object { $_.Status -in @('OK', 'PARTIAL') })
    [array]::Reverse($rows)
    $undoLog = Join-Path $OutDir "PC-Organize-Undo-$stamp.csv"
    Write-Host "Откат $($rows.Count) операций из $Undo" -ForegroundColor Yellow
    foreach ($r in $rows) {
        try {
            $res = Move-Entry $r.Final $r.Source $r.Type
            $status = $res.Status; $note = $res.Note
        } catch { $status = 'FAILED'; $note = $_.Exception.Message }
        Write-Host ("[{0}] {1} <- {2}" -f $status, $r.Source, $r.Final)
        [pscustomobject]@{ Time = (Get-Date).ToString('s'); Type = $r.Type; Source = $r.Final; Final = $r.Source; Status = $status; Note = $note } | Export-Csv -LiteralPath $undoLog -Append -NoTypeInformation -Encoding UTF8
    }
    Write-Host "Журнал отката: $undoLog" -ForegroundColor Green
    return
}

# ---------------------------------------------------------------- plan from file

if ($PlanFile) {
    if (-not (Test-Path -LiteralPath $PlanFile)) { throw "План не найден: $PlanFile" }
    $script:Plan = New-Object System.Collections.Generic.List[object]
    foreach ($r in (Import-Csv -LiteralPath $PlanFile -Encoding UTF8)) { $script:Plan.Add($r) }
}

# ---------------------------------------------------------------- print and export plan

$moves = @($script:Plan | Where-Object { $_.Action -eq 'MOVE' })
$deletes = @($script:Plan | Where-Object { $_.Action -eq 'DELETE?' })
$manual = @($script:Plan | Where-Object { $_.Action -eq 'MANUAL' })

Write-Host ''
Write-Host ("План: {0} перемещений ({1:N1} ГБ), {2} кандидатов на удаление ({3:N1} ГБ), {4} вручную" -f $moves.Count, (($moves | Measure-Object SizeMB -Sum).Sum / 1024), $deletes.Count, (($deletes | Measure-Object SizeMB -Sum).Sum / 1024), $manual.Count) -ForegroundColor Cyan
Write-Host ''
$byDest = $moves | ForEach-Object { [pscustomobject]@{ DestDir = (Split-Path $_.Destination -Parent); SizeMB = [double]$_.SizeMB } } | Group-Object DestDir | ForEach-Object {
    [pscustomobject]@{ Items = $_.Count; GB = [math]::Round(($_.Group | Measure-Object SizeMB -Sum).Sum / 1024, 2); Destination = $_.Name }
} | Sort-Object Destination
$byDest | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

if (-not $PlanFile) {
    $planCsv = Join-Path $OutDir "PC-Organize-Plan-$stamp.csv"
    $script:Plan | Export-Csv -LiteralPath $planCsv -NoTypeInformation -Encoding UTF8
    $md = New-Object System.Text.StringBuilder
    [void]$md.AppendLine("# План сортировки — $stamp")
    [void]$md.AppendLine('')
    [void]$md.AppendLine("Корень: ``$Root``. Перемещений: $($moves.Count) ($([math]::Round(($moves | Measure-Object SizeMB -Sum).Sum/1024,1)) ГБ). Ничего не удаляется: раздел «Кандидаты на удаление» — только список.")
    [void]$md.AppendLine('')
    [void]$md.AppendLine('## Куда что переедет')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('| Назначение | Элементов | ГБ |')
    [void]$md.AppendLine('| --- | --- | --- |')
    foreach ($b in $byDest) { [void]$md.AppendLine("| ``$($b.Destination)`` | $($b.Items) | $($b.GB) |") }
    [void]$md.AppendLine('')
    [void]$md.AppendLine('## Перемещения')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('| Группа | Откуда | Куда | МБ | Примечание |')
    [void]$md.AppendLine('| --- | --- | --- | --- | --- |')
    foreach ($m in ($moves | Sort-Object Group, Source)) { [void]$md.AppendLine("| $($m.Group) | ``$($m.Source)`` | ``$($m.Destination)`` | $($m.SizeMB) | $($m.Note) |") }
    [void]$md.AppendLine('')
    [void]$md.AppendLine('## Кандидаты на удаление (решает пользователь)')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('| Группа | Что | МБ | Почему |')
    [void]$md.AppendLine('| --- | --- | --- | --- |')
    foreach ($d in ($deletes | Sort-Object Group, Source)) { [void]$md.AppendLine("| $($d.Group) | ``$($d.Source)`` | $($d.SizeMB) | $($d.Note) |") }
    [void]$md.AppendLine('')
    [void]$md.AppendLine('## Вручную')
    [void]$md.AppendLine('')
    foreach ($m in $manual) { [void]$md.AppendLine("- ``$($m.Source)`` — $($m.Note)") }
    $planMd = Join-Path $OutDir "PC-Organize-Plan-$stamp.md"
    [System.IO.File]::WriteAllText($planMd, $md.ToString(), (New-Object System.Text.UTF8Encoding $true))
    Write-Host "План сохранён: $planCsv" -ForegroundColor Green
    Write-Host "               $planMd" -ForegroundColor Green
}

if (-not $Apply) {
    Write-Host ''
    Write-Host 'Пробный прогон: ничего не перемещено. Повторите с -Apply, чтобы выполнить план.' -ForegroundColor Yellow
    return
}

# ---------------------------------------------------------------- apply

$busy = @()
foreach ($p in 'codewhale-desktop', 'Telegram', 'Unity', 'Godot_v4.7.2-stable_win64', 'dbeaver') {
    if (Get-Process -Name $p -ErrorAction SilentlyContinue) { $busy += $p }
}
if ($busy) { Write-Host "Внимание: запущены $($busy -join ', ') — их файлы могут не перенестись (строки PARTIAL/FAILED в журнале)." -ForegroundColor Yellow }

$journal = Join-Path $OutDir "PC-Organize-Journal-$stamp.csv"
$ok = 0; $bad = 0
foreach ($m in $moves) {
    if (-not (Test-Path -LiteralPath $m.Source)) { continue }
    try {
        $res = Move-Entry $m.Source $m.Destination $m.Type
        $status = $res.Status; $final = $res.Final; $note = $res.Note
    } catch { $status = 'FAILED'; $final = $m.Destination; $note = $_.Exception.Message }
    if ($status -eq 'OK') { $ok++ } else { $bad++ }
    $color = $(if ($status -eq 'OK') { 'Green' } elseif ($status -eq 'SKIPPED') { 'DarkGray' } else { 'Red' })
    Write-Host ("[{0}] {1} -> {2} {3}" -f $status, $m.Source, $final, $note) -ForegroundColor $color
    [pscustomobject]@{ Time = (Get-Date).ToString('s'); Group = $m.Group; Type = $m.Type; Source = $m.Source; Final = $final; Status = $status; Note = $note } |
        Export-Csv -LiteralPath $journal -Append -NoTypeInformation -Encoding UTF8
}
Write-Host ''
Write-Host "Готово: $ok перенесено, $bad с проблемами. Журнал: $journal" -ForegroundColor Cyan
Write-Host "Откат: powershell -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -Undo `"$journal`"" -ForegroundColor DarkGray
