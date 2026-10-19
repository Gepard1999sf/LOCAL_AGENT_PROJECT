# get-kiwix-books.ps1 - downloads Kiwix books (.zim) into <DriveRoot>\kiwix_zim, which kiwix-serve
# and the ai-drive tools kiwix_search / kiwix_read read. Windows PowerShell 5.1, saved as UTF-8 with BOM.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File get-kiwix-books.ps1 -DriveRoot F:\AI_Drive
#   ... -Books wikipedia_en-simple_all_maxi,cs.stackexchange.com_en_all   (book name prefixes, see below)
#   ... -Category stack_exchange     (folder on download.kiwix.org/zim/; default: resolved per book)
#   ... -MaxGB 70 -Verify -DryRun
#
# A prefix is the file name without the release date: <project>_<lang>_<selection>[_<flavour>]
# (flavour maxi = with images, nopic = full text without images, mini = only the lead of each article).
# For each prefix the newest release in https://download.kiwix.org/zim/<category>/ is taken
# ('-' and '_' are treated alike, so wikipedia_en_simple_all_maxi finds wikipedia_en-simple_all_maxi).
# A name with a date (unix.stackexchange.com_en_all_2026-08) pins that release, e.g. when the newest
# one has reached only a slow mirror so far.
# Idempotent: files already present are skipped (an older release of the same book is kept and not
# replaced; delete it to update), an interrupted BITS job is picked up and finished on the next run.
# Downloads go through an asynchronous BITS job that the script waits for and completes (so the file
# gets its final name only when it is whole); after Ctrl+C the job keeps downloading in the background
# and the next run waits for it. -MaxGB caps the total of new downloads per run, -Verify checks SHA-256
# against the .meta4 metalink, -DryRun only resolves names and sizes. Exit code 1 if a download failed.
param(
    [string]$DriveRoot = "D:\AI_Drive",
    [string[]]$Books = @(
        # Plain-language explanations.
        "wiktionary_en-simple_all_nopic",
        "wikiversity_ru_all_maxi",
        "wikibooks_ru_all_maxi",
        "wikipedia_en-simple_all_maxi",
        "wikibooks_en_all_nopic",
        "wikipedia_ru_all_nopic",
        # Computer science, architecture and operations (devdocs books are a few MB each).
        "devdocs_en_python",
        "devdocs_en_javascript",
        "devdocs_en_http",
        "devdocs_en_git",
        "devdocs_en_bash",
        "devdocs_en_docker",
        "devdocs_en_nginx",
        "devdocs_en_postgresql",
        "devdocs_en_sqlite",
        "devdocs_en_redis",
        "devdocs_en_kubernetes",
        "devdocs_en_node",
        "devops.stackexchange.com_en_all",
        "networkengineering.stackexchange.com_en_all",
        "cs.stackexchange.com_en_all",
        "softwareengineering.stackexchange.com_en_all",
        "dba.stackexchange.com_en_all",
        "unix.stackexchange.com_en_all",
        "serverfault.com_en_all"
    ),
    [string]$Category = "",
    [double]$MaxGB = 70,
    [switch]$Verify,
    [switch]$DryRun
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$base = "https://download.kiwix.org/zim"
$catalogUrl = "https://library.kiwix.org/catalog/v2/entries"

function Step($text) { Write-Host ""; Write-Host "== $text" -ForegroundColor Cyan }
function Warn($text) { Write-Host "  $text" -ForegroundColor Yellow }
function Gb([double]$bytes) {
    if ($bytes -lt 1GB) { return "{0:N1} МБ" -f ($bytes / 1MB) }
    return "{0:N2} ГБ" -f ($bytes / 1GB)
}

function Get-Text($url) {
    $c = (Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 120).Content
    if ($c -is [byte[]]) { $c = [System.Text.Encoding]::UTF8.GetString($c) }
    return [string]$c
}

# Loose key for matching: case and '-' vs '_' do not matter (Kiwix renamed wikipedia_en_simple -> en-simple);
# the dash of the release date (_2026-09.zim) is kept so the date patterns below still apply.
function Loose([string]$s) { return ($s.ToLowerInvariant() -replace '-(?!\d{2}\.zim$)', '_') }

# Release date from a Kiwix file name: ..._2026-09.zim -> 2026-09.
function ZimDate([string]$name) { if ($name -match '_(\d{4}-\d{2})\.zim$') { return $Matches[1] } return "0000-00" }

# Directory listing of download.kiwix.org/zim/<category>/ as (Name, Size) rows; fetched once per run.
$script:listings = @{}
function Get-Listing([string]$cat) {
    if (-not $script:listings.ContainsKey($cat)) {
        $rows = @()
        try {
            $html = Get-Text "$base/$cat/"
            $rx = '<a href="([^"/?]+\.zim)">[^<]*</a>\s+\d{4}-\d{2}-\d{2} \d{2}:\d{2}\s+(\S+)'
            $rows = @([regex]::Matches($html, $rx) | ForEach-Object {
                [pscustomobject]@{ Name = $_.Groups[1].Value; Size = $_.Groups[2].Value }
            })
        } catch {
            Warn "не удалось получить листинг $base/$cat/ ($($_.Exception.Message))"
        }
        $script:listings[$cat] = $rows
    }
    return $script:listings[$cat]
}

# Category folder guessed from the book name; $null when unknown (then the OPDS catalog is asked).
function Guess-Category([string]$prefix) {
    $p = Loose $prefix
    if ($p -match '^(wikipedia|wikibooks|wikiversity|wiktionary|wikiquote|wikisource|wikinews|wikivoyage|devdocs|libretexts|freecodecamp|gutenberg|vikidia|phet|ted)[._]') {
        if ($Matches[1] -eq 'libretexts') { return 'libretexts' }
        return $Matches[1]
    }
    if ($p -match '(^|\.)(stackexchange\.com|serverfault\.com|superuser\.com|askubuntu\.com|stackoverflow\.com|mathoverflow\.net|stackapps\.com)_') { return 'stack_exchange' }
    return $null
}

# Category from the OPDS catalog: the acquisition link is .../zim/<category>/<file>.zim.meta4.
function Find-CategoryInCatalog([string]$prefix) {
    $book = $prefix -replace '_(maxi|nopic|mini)$', ''
    foreach ($name in @($book, $book.Replace('_simple_', '-simple_')) | Select-Object -Unique) {
        try {
            $xml = Get-Text "$catalogUrl`?name=$([uri]::EscapeDataString($name))&count=5"
            $m = [regex]::Match($xml, 'href="https?://[^"]*/zim/([^/"]+)/[^"]+\.zim\.meta4"')
            if ($m.Success) { return $m.Groups[1].Value }
        } catch {
            Warn "каталог library.kiwix.org не ответил ($($_.Exception.Message))"
            return $null
        }
    }
    return $null
}

# Newest file <prefix>_<YYYY-MM>.zim in a listing (or the release $pin); exact spelling first, then the loose one.
function Find-Newest($rows, [string]$prefix, [string]$pin = "") {
    $date = if ($pin) { [regex]::Escape($pin) } else { '\d{4}-\d{2}' }
    $exact = '^' + [regex]::Escape($prefix) + "_$date\.zim$"
    $hits = @($rows | Where-Object { $_.Name -match $exact })
    if ($hits.Count -eq 0) {
        $loose = '^' + [regex]::Escape((Loose $prefix)) + "_$date\.zim$"
        $hits = @($rows | Where-Object { (Loose $_.Name) -match $loose })
    }
    return $hits | Sort-Object @{ Expression = { ZimDate $_.Name } }, Name | Select-Object -Last 1
}

# Flavours of a book that only exists as _maxi/_nopic/_mini (used to hint the right prefix).
function Find-Variants($rows, [string]$prefix) {
    $rx = '^' + [regex]::Escape((Loose $prefix)) + '_(maxi|nopic|mini)_\d{4}-\d{2}\.zim$'
    return @($rows | Where-Object { (Loose $_.Name) -match $rx } | Group-Object { (Loose $_.Name) -replace '_\d{4}-\d{2}\.zim$', '' } |
        ForEach-Object { $_.Group | Sort-Object @{ Expression = { ZimDate $_.Name } } | Select-Object -Last 1 })
}

# Exact size and SHA-256 from the MirrorBrain metalink next to the file; HEAD as a fallback for the size.
function Get-FileInfo([string]$url) {
    $info = [pscustomobject]@{ Size = [int64]-1; Sha256 = $null }
    try {
        [xml]$meta = Get-Text "$url.meta4"
        $file = @($meta.metalink.file)[0]
        $info.Size = [int64]$file.size
        $sha = @($file.hash) | Where-Object { $_.type -eq 'sha-256' } | Select-Object -First 1
        if ($sha) { $info.Sha256 = $sha.'#text'.Trim().ToLowerInvariant() }
    } catch {
        try {
            $head = Invoke-WebRequest -Uri $url -Method Head -UseBasicParsing -TimeoutSec 60
            $info.Size = [int64]$head.Headers['Content-Length']
        } catch { }
    }
    return $info
}

# HTTP status from a BITS error code 0x8019xxxx (BG_E_HTTP_ERROR_xxx), 0 otherwise.
function HttpStatus($code) {
    $hex = '{0:X8}' -f $code
    if ($hex -match '^8019([0-9A-F]{4})$') { return [Convert]::ToInt32($Matches[1], 16) }
    return 0
}

# Waits for a BITS job, printing progress, and completes it so the file gets its final name.
# Returns the downloaded file's path, or $null when it is not done; a stalled job stays queued for the next run.
function Wait-Bits($job, [string]$label, [int64]$total) {
    $started = Get-Date; $startBytes = [int64]$job.BytesTransferred
    $lastBytes = $startBytes; $lastMove = Get-Date; $lastPrint = Get-Date; $resumes = 0
    while ($true) {
        # A job cancelled from outside (Remove-BitsTransfer, bitsadmin /reset) reports an empty state; look it up again.
        $job = Get-BitsTransfer -JobId $job.JobId -ErrorAction SilentlyContinue
        if (-not $job) { Warn "BITS-задание $label отменено извне"; return $null }
        $state = [string]$job.JobState
        if ($state -eq 'Transferred') {
            $local = @($job.FileList)[0].LocalName
            Complete-BitsTransfer -BitsJob $job | Out-Null
            return $local
        }
        if ($state -eq 'Error' -or $state -eq 'Suspended') {
            if ($state -eq 'Error') {
                $status = HttpStatus $job.InternalErrorCode
                Warn ("BITS: {0} (код 0x{1:X8}{2})" -f ([string]$job.ErrorDescription).Trim(), $job.InternalErrorCode, $(if ($status) { ", HTTP $status" } else { "" }))
                if ($status -ge 400 -and $status -lt 500) {
                    Warn "сервер отказал (HTTP $status), задание удалено"
                    Remove-BitsTransfer -BitsJob $job
                    return $null
                }
                if ($resumes -ge 5) { Warn "после 5 попыток задание оставлено в очереди BITS; повторный запуск скрипта его продолжит"; return $null }
                $resumes++
            }
            Resume-BitsTransfer -BitsJob $job -Asynchronous | Out-Null
        }
        if ($state -eq 'Cancelled' -or $state -eq 'Acknowledged') { Warn "BITS-задание в состоянии $state"; return $null }
        $bytes = [int64]$job.BytesTransferred
        if ($bytes -ne $lastBytes) { $lastBytes = $bytes; $lastMove = Get-Date }
        if (((Get-Date) - $lastMove).TotalMinutes -ge 30) {
            Warn "30 минут без прогресса (состояние $state); задание оставлено в очереди BITS, повторный запуск его продолжит"
            return $null
        }
        if (((Get-Date) - $lastPrint).TotalSeconds -ge 60) {
            $lastPrint = Get-Date
            $all = if ($job.BytesTotal -gt 0 -and $job.BytesTotal -lt [uint64]::MaxValue) { [int64]$job.BytesTotal } else { $total }
            $speed = ($bytes - $startBytes) / [math]::Max(1, ((Get-Date) - $started).TotalSeconds) / 1MB
            $pct = if ($all -gt 0) { 100.0 * $bytes / $all } else { 0 }
            Write-Host ("  {0}: {1} из {2} ({3:N1}%), {4:N1} МБ/с, {5}" -f $label, (Gb $bytes), (Gb $all), $pct, $speed, $state)
        }
        Start-Sleep -Seconds 5
    }
}

# Downloads $url to $dest: BITS (resumable, picks up a job left by an earlier run), else curl.exe -C -.
function Download-Zim([string]$url, [string]$dest, [int64]$size) {
    $name = Split-Path $dest -Leaf
    $job = $null
    try {
        $job = @(Get-BitsTransfer -Name $name -ErrorAction SilentlyContinue) |
            Sort-Object { [int64]$_.BytesTransferred } -Descending | Select-Object -First 1
        if ($job) {
            Write-Host ("  продолжаю BITS-задание: {0} из {1} уже скачано" -f (Gb $job.BytesTransferred), (Gb $size))
        } else {
            $job = Start-BitsTransfer -Source $url -Destination $dest -DisplayName $name -Description $url `
                -Asynchronous -Priority Foreground -RetryInterval 60
        }
        $local = Wait-Bits $job $name $size
        if (-not $local) { return $false }
        # A job started by init-ai-drive.ps1 for the same file writes to <name>.part.
        if ($local -ne $dest -and (Test-Path $local)) { Move-Item $local $dest -Force }
        return (Test-Path $dest)
    } catch {
        Warn "BITS не сработал ($($_.Exception.Message))"
        # A job that already exists keeps downloading in the background; a second copy via curl would race it.
        if ($job) { Warn "BITS-задание '$name' осталось в очереди; повторный запуск скрипта его продолжит"; return $false }
    }
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if (-not $curl) { Warn "curl.exe не найден; файл не скачан"; return $false }
    Write-Host "  пробую curl.exe (докачка с места обрыва)"
    & $curl.Source -L --fail --retry 5 --retry-delay 15 -C - -o "$dest.part" $url
    if ($LASTEXITCODE -ne 0) { Warn "curl.exe завершился с кодом $LASTEXITCODE; повторный запуск докачает $dest.part"; return $false }
    Move-Item "$dest.part" $dest -Force
    return $true
}

if (-not (Test-Path (Split-Path $DriveRoot -Qualifier))) { throw "Диск $(Split-Path $DriveRoot -Qualifier) не подключён" }
$zimDir = Join-Path $DriveRoot "kiwix_zim"
New-Item -ItemType Directory -Force -Path $zimDir | Out-Null

# -File passes "-Books a,b" as one string: split on commas and spaces.
$wanted = @($Books | ForEach-Object { $_ -split '[,\s]+' } | Where-Object { $_ } | ForEach-Object { $_ -replace '\.zim$', '' })
$budget = [int64]($MaxGB * 1GB)
$planned = [int64]0
$report = @()

Step "Книги Kiwix -> $zimDir (лимит новых загрузок $MaxGB ГБ$(if ($DryRun) { ', только проверка' }))"
foreach ($entry in $wanted) {
    Write-Host ""
    Write-Host "-- $entry"
    $prefix = $entry; $pin = ""
    if ($entry -match '^(.+)_(\d{4}-\d{2})$') { $prefix = $Matches[1]; $pin = $Matches[2] }
    $cat = if ($Category) { $Category } else { Guess-Category $prefix }
    $hit = $null
    if ($cat) { $hit = Find-Newest (Get-Listing $cat) $prefix $pin }
    if (-not $hit -and -not $Category) {
        $found = Find-CategoryInCatalog $prefix
        if ($found -and $found -ne $cat) { $cat = $found; $hit = Find-Newest (Get-Listing $cat) $prefix $pin }
    }
    if (-not $hit) {
        $variants = if ($cat -and -not $pin) { @(Find-Variants (Get-Listing $cat) $prefix) } else { @() }
        if ($variants.Count -gt 0) {
            Warn "НЕ НАЙДЕНО: '$prefix' есть только в вариантах, укажите один из них:"
            $variants | ForEach-Object { Warn ("    {0}  ({1})" -f ($_.Name -replace '_\d{4}-\d{2}\.zim$', ''), $_.Size) }
        } else {
            $where = if ($Category) { "$base/$Category/ (категория задана -Category)" }
                     elseif ($cat) { "$base/$cat/ и в каталоге library.kiwix.org" }
                     else { "каталоге library.kiwix.org (категорию определить не удалось)" }
            Warn "НЕ НАЙДЕНО: '$entry' нет в $where"
        }
        $report += [pscustomobject]@{ Book = $entry; File = "-"; Size = "-"; Status = "не найдено" }
        continue
    }
    $url = "$base/$cat/$($hit.Name)"
    $dest = Join-Path $zimDir $hit.Name
    $info = Get-FileInfo $url
    $sizeText = if ($info.Size -gt 0) { Gb $info.Size } else { "~$($hit.Size)" }
    Write-Host "  $($hit.Name)  $sizeText  ($cat)"

    if (Test-Path $dest) {
        $len = (Get-Item $dest).Length
        if ($info.Size -gt 0 -and $len -ne $info.Size) {
            Warn ("уже есть, но размер {0} байт, а на сервере {1}; удалите файл, чтобы скачать заново" -f $len, $info.Size)
            $report += [pscustomobject]@{ Book = $entry; File = $hit.Name; Size = $sizeText; Status = "есть (размер не совпадает)" }
        } else {
            Write-Host "  уже есть"
            $report += [pscustomobject]@{ Book = $entry; File = $hit.Name; Size = $sizeText; Status = "есть" }
        }
        continue
    }
    $older = Get-ChildItem $zimDir -File -Filter "*.zim" | Where-Object {
        (Loose $_.Name) -match ('^' + [regex]::Escape((Loose $prefix)) + '_\d{4}-\d{2}\.zim$') } | Select-Object -First 1
    if ($older) {
        Warn "уже есть другой выпуск $($older.Name); для обновления удалите его и запустите скрипт снова"
        $report += [pscustomobject]@{ Book = $entry; File = $older.Name; Size = (Gb $older.Length); Status = "есть другой выпуск" }
        continue
    }
    $need = if ($info.Size -gt 0) { $info.Size } else { [int64]0 }
    if ($planned + $need -gt $budget) {
        Warn ("ПРОПУЩЕНО: превысит лимит -MaxGB {0} (уже запланировано {1})" -f $MaxGB, (Gb $planned))
        $report += [pscustomobject]@{ Book = $entry; File = $hit.Name; Size = $sizeText; Status = "пропущено (лимит)" }
        continue
    }
    $free = (New-Object System.IO.DriveInfo((Split-Path $zimDir -Qualifier))).AvailableFreeSpace
    if ($need + 2GB -gt $free) {
        Warn ("ПРОПУЩЕНО: на диске свободно {0}, нужно {1} + 2 ГБ запаса" -f (Gb $free), (Gb $need))
        $report += [pscustomobject]@{ Book = $entry; File = $hit.Name; Size = $sizeText; Status = "пропущено (нет места)" }
        continue
    }
    $planned += $need
    if ($DryRun) {
        $report += [pscustomobject]@{ Book = $entry; File = $hit.Name; Size = $sizeText; Status = "будет скачано" }
        continue
    }

    Write-Host "  качаю $url"
    $ok = Download-Zim $url $dest $info.Size
    $status = "скачано"
    if (-not $ok) {
        $status = "не скачано"
    } elseif ($info.Size -gt 0 -and (Get-Item $dest).Length -ne $info.Size) {
        Warn ("размер {0} байт, ожидалось {1}; файл переименован в .bad" -f (Get-Item $dest).Length, $info.Size)
        Move-Item $dest "$dest.bad" -Force
        $status = "ошибка размера"
    } elseif ($Verify -and $info.Sha256) {
        Write-Host "  проверяю SHA-256"
        $hash = (Get-FileHash -Algorithm SHA256 -Path $dest).Hash.ToLowerInvariant()
        if ($hash -ne $info.Sha256) {
            Warn "SHA-256 не совпал ($hash, ожидалось $($info.Sha256)); файл переименован в .bad"
            Move-Item $dest "$dest.bad" -Force
            $status = "ошибка SHA-256"
        } else {
            $status = "скачано, SHA-256 совпал"
        }
    }
    Write-Host "  $status"
    $report += [pscustomobject]@{ Book = $entry; File = $hit.Name; Size = $sizeText; Status = $status }
}

Step "Итог"
$report | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
$missing = @($report | Where-Object { $_.Status -eq "не найдено" })
if ($missing.Count -gt 0) { Warn "Не найдено книг: $($missing.Count) - проверьте имена на https://library.kiwix.org" }
if (@($report | Where-Object { $_.Status -like "скачано*" }).Count -gt 0) {
    Write-Host "  Перезапустите kiwix-serve (CodeWhale Desktop: Диск знаний -> Kiwix), иначе новые книги не видны в поиске."
}
$failed = @($report | Where-Object { $_.Status -like "не скачано" -or $_.Status -like "ошибка*" })
if ($failed.Count -gt 0) { Warn "Не скачано: $($failed.Count); запустите скрипт ещё раз, он продолжит с места обрыва"; exit 1 }
