# Invoke-PcRecycle.ps1 - sends the "DELETE?" candidates of an Invoke-PcOrganize plan to the
# Recycle Bin (never a permanent delete: the user empties the bin). Dry run by default.
# Items larger than the volume's Recycle Bin limit are skipped and reported so the user can
# delete them by hand. Windows PowerShell 5.1; saved as UTF-8 with BOM.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\pc-organize\Invoke-PcRecycle.ps1 -PlanFile "<Desktop>\PC-Audit\PC-Organize-Plan-<date>.csv"
#   ... -Apply            actually move the items to the Recycle Bin
#   ... -Only <regex>     limit to sources matching the pattern
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$PlanFile,
    [switch]$Apply,
    [string]$Only,
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName Microsoft.VisualBasic
$stamp = (Get-Date).ToString('yyyy-MM-dd_HH-mm')
if (-not $OutDir) { $OutDir = Join-Path ([Environment]::GetFolderPath('Desktop')) 'PC-Audit' }
if (-not (Test-Path -LiteralPath $PlanFile)) { throw "План не найден: $PlanFile" }

# Never recycle these, whatever the plan says.
$protected = @('C:\', 'C:\Windows', 'C:\Program Files', 'C:\Program Files (x86)', 'C:\ProgramData', 'C:\Users', $env:USERPROFILE,
    (Join-Path $env:USERPROFILE 'AppData'), (Join-Path $env:USERPROFILE 'Desktop'), (Join-Path $env:USERPROFILE 'Documents'),
    (Join-Path $env:USERPROFILE 'Downloads'), 'D:\', 'F:\')

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

# Recycle Bin capacity per volume (MB), from Explorer's BitBucket settings; default 10 % of the volume if unknown.
$binLimit = @{}
foreach ($v in (Get-Volume -ErrorAction SilentlyContinue | Where-Object DriveLetter)) {
    $guid = ($v.UniqueId -replace '.*\{([0-9a-f-]+)\}.*', '$1')
    $key = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{$guid}"
    $mb = $null
    if (Test-Path $key) { $mb = (Get-ItemProperty $key).MaxCapacity }
    if (-not $mb) { $mb = [math]::Round($v.Size / 1MB * 0.1) }
    $binLimit["$($v.DriveLetter):"] = [long]$mb * 1MB
}

$rows = @(Import-Csv -LiteralPath $PlanFile -Encoding UTF8 | Where-Object { $_.Action -eq 'DELETE?' })
if ($Only) { $rows = @($rows | Where-Object { $_.Source -imatch $Only }) }

$work = @()
foreach ($r in $rows) {
    $src = $r.Source.TrimEnd('\')
    $status = 'RECYCLE'; $note = $r.Note
    if (-not (Test-Path -LiteralPath $src)) { $status = 'GONE'; $note = 'уже не существует' }
    elseif ($protected -contains $src -or $src.Length -le 3) { $status = 'PROTECTED'; $note = 'системная или корневая папка' }
    else {
        $item = Get-Item -LiteralPath $src -Force
        $bytes = $(if ($item.PSIsContainer) { Get-TreeBytes $src } else { $item.Length })
        $limit = $binLimit[$src.Substring(0, 2).ToUpperInvariant()]
        if ($limit -and $bytes -gt $limit) { $status = 'TOO-BIG'; $note = ("{0:N1} ГБ больше лимита Корзины {1:N1} ГБ — удалить вручную (Shift+Delete)" -f ($bytes / 1GB), ($limit / 1GB)) }
    }
    $work += [pscustomobject]@{ Status = $status; Type = $(if (Test-Path -LiteralPath $src -PathType Container) { 'dir' } else { 'file' }); Source = $src; MB = $(if ($status -in 'RECYCLE', 'TOO-BIG') { [math]::Round($bytes / 1MB, 1) } else { 0 }); Note = $note }
}

$work | Sort-Object Status, Source | Format-Table Status, MB, Source, Note -AutoSize | Out-String -Width 220 | Write-Host
$toDo = @($work | Where-Object Status -eq 'RECYCLE')
Write-Host ("В Корзину: {0} элементов, {1:N1} ГБ. Пропущено как слишком большие: {2}." -f $toDo.Count, (($toDo | Measure-Object MB -Sum).Sum / 1024), @($work | Where-Object Status -eq 'TOO-BIG').Count) -ForegroundColor Cyan

if (-not $Apply) { Write-Host 'Пробный прогон: ничего не удалено. Добавьте -Apply.' -ForegroundColor Yellow; return }

$journal = Join-Path $OutDir "PC-Recycle-Journal-$stamp.csv"
foreach ($w in $toDo) {
    if (-not (Test-Path -LiteralPath $w.Source)) { continue }
    try {
        if ($w.Type -eq 'dir') {
            [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($w.Source, [Microsoft.VisualBasic.FileIO.UIOption]::OnlyErrorDialogs, [Microsoft.VisualBasic.FileIO.RecycleOption]::SendToRecycleBin)
        } else {
            [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($w.Source, [Microsoft.VisualBasic.FileIO.UIOption]::OnlyErrorDialogs, [Microsoft.VisualBasic.FileIO.RecycleOption]::SendToRecycleBin)
        }
        $status = 'RECYCLED'; $note = ''
    } catch { $status = 'FAILED'; $note = $_.Exception.Message }
    Write-Host ("[{0}] {1} {2}" -f $status, $w.Source, $note) -ForegroundColor $(if ($status -eq 'RECYCLED') { 'Green' } else { 'Red' })
    [pscustomobject]@{ Time = (Get-Date).ToString('s'); Type = $w.Type; Source = $w.Source; MB = $w.MB; Status = $status; Note = $note } |
        Export-Csv -LiteralPath $journal -Append -NoTypeInformation -Encoding UTF8
}
Write-Host "Журнал: $journal. Место освободится после очистки Корзины (правый клик по Корзине → Очистить)." -ForegroundColor Cyan
