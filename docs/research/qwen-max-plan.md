# Qwen на максимум: RTX 5060 Laptop 8 ГБ + 32 ГБ ОЗУ

Синтез на 2026-10-19 (четыре исследования + проверки без GPU); исправляет
`rtx5060-8gb-model-plan.md`. Без замера — «оценка».

## 0. Коротко

1. **Агент — `qwen3.5:9b` без проектора зрения:** сильнейшая Qwen, влезающая в 8 ГБ. На модель
   ≈ 5,9 ГиБ, поэтому контекст 32k, запасной вариант IQ4_XS.
2. **Потолок качества — MoE `Qwen3.6-35B-A3B` UD-IQ4_XS:** эксперты уходят в ОЗУ сами (`--fit`);
   цена — ОЗУ и медленный первый ход.
3. **Бесплатно (проверено на mock):** `reasoning_effort = "none"` даёт `think: false` (`"auto"` —
   `true`); без goal-инструментов ≈ −1k токенов на запрос; текстовый вариант — +0,6–0,9 ГиБ VRAM.
4. **«~10k токенов накладных» устарело:** с текущим конфигом ≈ 5,3k (§ 5).
5. **Сэмплинг:** Ollama `/v1` ставит temp и top_p = 1,0, если поля не пришли; так все 143 запроса
   CodeWhale в логе.

## 1. Рычаги по убыванию отдачи

| # | Рычаг | Качество | Скорость | Цена / риск | Увер. | Как |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Агент → Qwen3.5-9B Q4_K_M, текстовый вариант | ++ BFCL-V4 66,1 vs 50,3 (3.5-4B); AA v4.0 32 vs 18 (Qwen3-4B-2507) | −20…−50 % (оценка) | 6,6 ГБ; 32k впритык; баги парсера #16383/#18563 | сред. | § 3, иначе IQ4_XS |
| 2 | Qwen3.6-35B-A3B IQ4_XS, эксперты в ОЗУ | +++ SWE-V 73,4; AA v4.3.2 18 vs 11 (9B); меньше галлюцинаций | 25–40 ток/с; 1-й ход 20–50 с (оценка) | 17,7 ГБ; 14–19 ГБ ОЗУ | низ.–сред. | без `num_gpu` |
| 3 | Thinking по шагу | ±: у `qwen3.5-4b` think 4,33 → 3,60 и 8 → 46 с | токенов в 2–5 раз меньше | плану think может быть нужен | выс. (механизм) | `reasoning_effort = "none"` |
| 4 | Без mmproj | — | +0,6–0,9 ГиБ VRAM | нет скриншотов | сред. | § 3.1 |
| 5 | Длинный контекст на гибридах | меньше компакций | ток/с падает слабее | — | выс. | KV q8_0/ток: 17 КиБ (3.5), 10,6 (35B), 76,5 (qwen3-4b) |
| 6 | Без goal-инструментов | меньше меню (не замерено) | −20 % первого запроса | нет `*_goal` | выс. (замер) | § 5 |
| 7 | Сэмплинг Qwen (0,6–0,7 / 0,8–0,95 / top_k 20) | + (не замерено) | — | прокси или llama-server | факт — выс. | прогон `qm_t1` |
| 8 | `LLAMA_ARG_FIT_TARGET`, `LLAMA_ARG_CACHE_RAM` | стабильность | — | глобальный env | сред. | § 3.6, § 4 |
| 9 | `num_batch 1024` (MoE), `ngram-mod`, MTP 4B | — | + (не замерено) | `9b-mtp` 9,0 ГБ — не влезает | низ. | после замера |
| 10 | T-lite-it-2.1 для русского | Ru Arena Hard 83,9 vs 57,2 | как qwen3:8b | 5,0 ГБ; контекст 16–24k | сред. | опция |
| 11 | Qwen3.8-27B | потолок, AA v4.3.2 — 34 | 6–9 ток/с (оценка) | reasoning xhigh по умолчанию | низ. | эксперт, think off |
| 12 | llama-server напрямую | стрим tool-calls, сэмплеры | — | ломает менеджер моделей, ComfyUI | сред. | позже |

**Не стоит:** 9B в Q6/Q8 (+1–3 % при 10–22 ток/с); теги `nvfp4`/`mxfp8` (MLX, не CUDA); gpt-oss-20b,
Gemma 4, GigaChat3.1; `qwen2.5-coder:7b` для объяснений (зацикливалась в 3 из 10); `gpt-4o:latest`
(это `deepseek-r1-8k`).

### 1.1 Что было неверно

**План `rtx5060-8gb-model-plan.md`:**
- **Размер `qwen3.5:9b`** — 6,6 ГБ (5,68 веса + 0,92 mmproj), а не 5,5.
- **KV 9B** — 32 / 17 КиБ на токен в f16 / q8_0 (8 слоёв внимания × 4 головы × 256); 20 КиБ было взято у 35B.
- **Бюджет VRAM** — не 6,8 ГиБ (правило 85 %), а `available − 1 ГиБ` (запас `--fit`): в логе
  6,9 → ≈ 5,9. Замер против плана: `qwen3-8b-16k` 5,85 vs 6,8; `qwen3-4b-32k` 4,90 vs 5,1.
  9B Q4_K_M при 32k (6,2–6,6) под вопросом, IQ4_XS ≈ 5,9.
- **MoE:** прав research [16] (`fit.cpp` выносит экспертов в ОЗУ), research [3] устарел.
- **NVFP4/MXFP4** — MLX, не CUDA; **MTP 9B** — 9,0 ГБ, не влезает; **Ollama** — 0.40.4, не 0.40.3.
- **`qwen2.5-coder:7b`** снята с роли запасного агента: агентом не проверялась.

**Между исследователями:**
- **mmap.** На Windows+CUDA Ollama выключает mmap (`--load-mode none`): эксперты лежат в приватной
  ОЗУ, вытеснения mmap-страниц не будет.
- **`qwen3.6:35b-a3b`** весит 23–24 ГБ, а не 22.
- **Индексы AA** v4.0 и v4.3.2 между собой не сравниваются.
- **«38–40 ток/с у 9B»** — оценка от 87 ток/с на пустом промпте. С промптом 3,2k даже 4B даёт 66.

## 2. Набор моделей для бенчмарка

| # | Вариант | Источник | Скачать | Роль | Треки |
| --- | --- | --- | ---: | --- | --- |
| 1 | `qwen3-4b-32k` | установлена | 0 | baseline | a b c |
| 2 | `qwen3.5-4b-text-64k` | `qwen3.5:4b` (есть) | 0 | «Godot открыт», объяснения | a b c |
| 3 | `qwen3.5-9b-text-32k` | `qwen3.5:9b` | 6,6 ГБ | главный кандидат | a b c |
| 4 | `qwen3.6-35b-iq4-64k` и `…-go` (тот же блоб, Go-парсер) | `hf.co/unsloth/Qwen3.6-35B-A3B-GGUF:UD-IQ4_XS` | 17,73 ГБ | MoE | a b c |
| 5 | `qwen3.8-27b-iq3-16k` | `hf.co/unsloth/Qwen3.8-27B-GGUF:UD-IQ3_XXS` | 10,93 ГБ | потолок, think off | b c |
| 6 | `deepseek-r1-7b-32k` | установлена | 0 | эталон, KV **f16** | a (×1) b c |
| опц. | `qwen3.5-9b-iq4-32k` | `hf.co/unsloth/Qwen3.5-9B-GGUF:IQ4_XS` | 5,17 ГБ | если № 3 не 100 % GPU | a c |
| опц. | `t-lite-2.1-24k` | `hf.co/t-tech/T-lite-it-2.1-GGUF:Q4_K_M` | 5,03 ГБ | русский | b |

Обязательные скачивания — 35,3 ГБ, с опциями — 45,5 ГБ. Скачивать только с разрешения пользователя.

```powershell
ollama pull qwen3.5:9b
ollama pull hf.co/unsloth/Qwen3.6-35B-A3B-GGUF:UD-IQ4_XS
ollama pull hf.co/unsloth/Qwen3.8-27B-GGUF:UD-IQ3_XXS
# optional:
ollama pull hf.co/unsloth/Qwen3.5-9B-GGUF:IQ4_XS
ollama pull hf.co/t-tech/T-lite-it-2.1-GGUF:Q4_K_M
```

## 3. Протокол бенчмарка (один агент, последовательно)

Блоки самодостаточны (переменные между вызовами не живут); циклы дольше 10 мин — фоном; всё в
`D:\AI\cw-bench\qwen-max`. Порядок: 3.0 → 3.1 → 3.2 → (c), (b), (a) на q8_0 → f16 (3.6) → опции →
возврат трея.

### 3.0 Предусловия

Есть разрешение пользователя, Godot и ComfyUI закрыты, зарядка подключена, чужих CUDA-процессов нет.

```powershell
New-Item -ItemType Directory -Force -Path 'D:\AI\cw-bench\qwen-max\a' | Out-Null
powershell -NoProfile -File 'D:\AI\cw-bench\explain-eval\wait_gpu.ps1' -TimeoutMinutes 120
nvidia-smi --query-gpu=memory.total,memory.used,memory.free --format=csv
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv
ollama ps
powercfg /getactivescheme
```

9B Q4_K_M при 32k нужно ≥ 7,5 ГиБ `memory.free` в простое.

### 3.1 Текстовые варианты

FROM берётся по самому большому блобу (это веса), проектор отбрасывается. Сэмплеры — пресет
Qwen для кода.

```powershell
function New-TextVariant([string]$Source, [string]$Name, [string[]]$Lines) {
    $froms = @(ollama show --modelfile $Source | Where-Object { $_ -like 'FROM *' } | ForEach-Object { $_.Substring(5).Trim() })
    $text = $froms | Sort-Object { (Get-Item -LiteralPath $_).Length } -Descending | Select-Object -First 1
    $file = "D:\AI\cw-bench\qwen-max\$Name.Modelfile"
    Set-Content -LiteralPath $file -Value (@("FROM $text") + $Lines) -Encoding ascii
    ollama create $Name -f $file
    ollama show $Name
}
$q35 = @('RENDERER qwen3.5', 'PARSER qwen3.5')
$smp = @('PARAMETER temperature 0.6', 'PARAMETER top_p 0.95', 'PARAMETER top_k 20', 'PARAMETER presence_penalty 0')
New-TextVariant 'qwen3.5:4b' 'qwen3.5-4b-text-64k' ($q35 + $smp + 'PARAMETER num_ctx 65536')
New-TextVariant 'qwen3.5:9b' 'qwen3.5-9b-text-32k' ($q35 + $smp + 'PARAMETER num_ctx 32768')
New-TextVariant 'hf.co/unsloth/Qwen3.6-35B-A3B-GGUF:UD-IQ4_XS' 'qwen3.6-35b-iq4-64k' ($smp + 'PARAMETER num_ctx 65536')
New-TextVariant 'hf.co/unsloth/Qwen3.6-35B-A3B-GGUF:UD-IQ4_XS' 'qwen3.6-35b-iq4-64k-go' ($q35 + $smp + 'PARAMETER num_ctx 65536')
New-TextVariant 'hf.co/unsloth/Qwen3.8-27B-GGUF:UD-IQ3_XXS' 'qwen3.8-27b-iq3-16k' ($smp + 'PARAMETER num_ctx 16384')
```

В Capabilities не должно быть `vision`. У hf.co-вариантов шаблон из GGUF (jinja); пара `-64k` /
`-64k-go` сравнивает его с Go-парсером. Опции тем же способом: `qwen3.5-9b-iq4-32k` (`$smp` +
32768), `t-lite-2.1-24k` (24576, temp 0,7, top_p 0,8), `…-b1024` (+ `num_batch 1024`).

### 3.2 Конфиги CodeWhale (копии; проверены TOML-разбором и `merge_codewhale_config`)

```powershell
$bench = 'D:\AI\cw-bench\qwen-max'
$src = Get-Content -LiteralPath "$env:USERPROFILE\.codewhale\config.toml" -Raw -Encoding UTF8
$win = @'
[providers.ollama.model_context_windows]
"qwen3.5-4b-text-64k" = 65536
"qwen3.5-9b-text-32k" = 32768
"qwen3.5-9b-iq4-32k" = 32768
"qwen3.6-35b-iq4-64k" = 65536
"qwen3.6-35b-iq4-64k-go" = 65536
'@
$goals = @'

[tools.overrides]
"create_goal" = { type = "disabled" }
"get_goal" = { type = "disabled" }
"update_goal" = { type = "disabled" }
'@
$base = $src.Replace('[providers.ollama.model_context_windows]', $win.TrimEnd())
Set-Content -LiteralPath "$bench\config-base.toml" -Value $base -Encoding UTF8
Set-Content -LiteralPath "$bench\config-lean.toml" -Value ($base.Replace('reasoning_effort = "auto"', 'reasoning_effort = "none"') + $goals) -Encoding UTF8
Set-Content -LiteralPath "$bench\config-think.toml" -Value ($base + $goals) -Encoding UTF8
```

Профили: `base` — конфиг пользователя как есть; `lean` — `think: false` и без goal-инструментов;
`think` — `lean` с `think: true`.

### 3.3 Трек (c): скорость и VRAM

256 токенов, `temperature 0`, `seed 42`, `think: false`. Случаи: `short-cold` (с загрузкой),
`short-warm`, `long-cold` (prefill ≈ 13k), `long-warm` (генерация на 13k контекста).
`eval_tok_s` = `eval rate` из `ollama run --verbose`; ручная сверка:
`ollama run qwen3.5-9b-text-32k --verbose --think=false "Привет"`.

```powershell
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$bench = 'D:\AI\cw-bench\qwen-max'
$code = (git -C 'F:\AI\CodeWhale\LOCAL_AGENT_PROJECT' show 453cb7f:codewhale-desktop/src-tauri/src/models.rs) -join "`n"
Set-Content -LiteralPath "$bench\long13k.txt" -Value ("Перескажи по-русски в пяти пунктах, что делает этот файл.`n`n" + $code) -Encoding UTF8
```

```powershell
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$bench = 'D:\AI\cw-bench\qwen-max'; $api = 'http://127.0.0.1:11434'; $kv = 'q8_0'
$models = @('qwen3-4b-32k', 'qwen3.5-4b-text-64k', 'qwen3.5-9b-text-32k', 'qwen3.6-35b-iq4-64k', 'qwen3.8-27b-iq3-16k')
$short = 'Напиши на GDScript функцию, которая возвращает n-е число Фибоначчи, и коротко объясни её по-русски.'
$long = Get-Content -LiteralPath "$bench\long13k.txt" -Raw -Encoding UTF8
function Invoke-Ollama([string]$Path, [hashtable]$Body) {
    $json = $Body | ConvertTo-Json -Depth 6
    Invoke-RestMethod -Uri "$api$Path" -Method Post -ContentType 'application/json; charset=utf-8' -Body ([Text.Encoding]::UTF8.GetBytes($json)) -TimeoutSec 3600
}
foreach ($m in $models) {
    Invoke-Ollama '/api/generate' @{ model = $m; keep_alive = 0 } | Out-Null
    foreach ($case in @(@('short-cold', $short), @('short-warm', $short), @('long-cold', $long), @('long-warm', $long))) {
        $r = Invoke-Ollama '/api/generate' @{ model = $m; prompt = $case[1]; stream = $false; think = $false; keep_alive = '10m'; options = @{ temperature = 0; seed = 42; num_predict = 256 } }
        $ps = @((Invoke-RestMethod -Uri "$api/api/ps").models | Where-Object { $_.name -like "$m*" })[0]
        $used = (nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits) -join ''
        [pscustomobject]@{
            model = $m; kv = $kv; case = $case[0]; prompt_tokens = $r.prompt_eval_count
            prefill_tok_s = if ($r.prompt_eval_duration) { [math]::Round($r.prompt_eval_count * 1e9 / $r.prompt_eval_duration, 1) } else { $null }
            eval_tok_s = [math]::Round($r.eval_count * 1e9 / $r.eval_duration, 1)
            load_s = [math]::Round($r.load_duration / 1e9, 1)
            size_gib = [math]::Round($ps.size / 1GB, 2); vram_gib = [math]::Round($ps.size_vram / 1GB, 2)
            ctx = $ps.context_length; smi_used_mib = $used
        } | Export-Csv -LiteralPath "$bench\c-speed.csv" -Append -NoTypeInformation -Encoding UTF8
    }
    Invoke-Ollama '/api/generate' @{ model = $m; keep_alive = 0 } | Out-Null
}
Import-Csv -LiteralPath "$bench\c-speed.csv" | Format-Table -AutoSize
```

После каждой модели выписать из `%LOCALAPPDATA%\Ollama\server.log` строки `projected to use`,
`will leave` и `offloaded N/M layers`. Для MoE — ещё пик commit ОЗУ.

Если у 9B `vram_gib < size_gib`: скачать IQ4_XS и повторить треки (c) и (a).

### 3.4 Трек (b): объяснения по-русски с Mermaid

Стенд `D:\AI\cw-bench\explain-eval`: 5 вопросов (вектор, переменная, серверная архитектура,
x² − 5x + 6 = 0, рекурсия fact(3)), навык `explain-concepts`, `/api/chat`, `temperature 0.3`,
2 сэмпла на вопрос.

```powershell
Set-Location 'D:\AI\cw-bench\explain-eval'
$m = 'qwen3-4b-32k,qwen3.5-4b-text-64k,qwen3.5-9b-text-32k,qwen3.6-35b-iq4-64k,qwen3.8-27b-iq3-16k'
py -I -X utf8 .\run_eval.py --tag qm_b1 --think false --models $m
py -I -X utf8 .\run_eval.py --tag qm_b2 --think false --models $m
py -I -X utf8 .\run_eval.py --tag qm_think --think true --models 'qwen3.5-9b-text-32k,qwen3.6-35b-iq4-64k' --num-predict 6000
(Get-Content -LiteralPath .\run_eval.py -Raw).Replace('"temperature": 0.3,', '"temperature": 1.0, "top_p": 1.0,') | Set-Content -LiteralPath .\run_eval_t1.py -Encoding UTF8
py -I -X utf8 .\run_eval_t1.py --tag qm_t1 --think false --models 'qwen3.5-9b-text-32k'
$env:CW_E2E_CHROMIUM = 'C:\Users\Stryker\AppData\Local\ms-playwright\chromium-1234\chrome-win64\chrome.exe'
node md_check.mjs raw/qm_b1.json raw/qm_b2.json raw/qm_think.json raw/qm_t1.json
$env:USE_MD = '1'; node render.js raw/qm_b1.json raw/qm_b2.json raw/qm_think.json raw/qm_t1.json
```

`qm_t1` воспроизводит сэмплинг CodeWhale через `/v1` (temp и top_p = 1,0). Разница с `qm_b1` —
цена рычага 7.

### 3.5 Трек (a): инструменты через CodeWhale

Живой сценарий воды: `water.gdshader` → MCP `godot_check` → правки до прохождения; без `--render`.

```powershell
$bench = 'D:\AI\cw-bench\qwen-max'; $repo = 'F:\AI\CodeWhale\LOCAL_AGENT_PROJECT'
$portable = 'F:\AI\CodeWhale\portable\CodeWhale-Desktop_0.1.0_x64-portable'
$plan = @(
    @('qwen3-4b-32k', 'base', 3), @('qwen3-4b-32k', 'lean', 3), @('qwen3.5-4b-text-64k', 'lean', 3),
    @('qwen3.5-9b-text-32k', 'lean', 3), @('qwen3.5-9b-text-32k', 'think', 2),
    @('qwen3.6-35b-iq4-64k', 'lean', 3), @('qwen3.6-35b-iq4-64k', 'think', 1), @('qwen3.6-35b-iq4-64k-go', 'lean', 2)
)
foreach ($p in $plan) {
    for ($i = 1; $i -le $p[2]; $i++) {
        $out = "$bench\a\$($p[0])-$($p[1])-$i"
        py -I -X utf8 "$repo\codewhale-desktop\e2e\fixtures\live\final_test_water.py" --codewhale "$portable\codewhale-app-server.exe" --desktop "$portable\codewhale-desktop.exe" --godot 'F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe' --project "$repo\Godot\WaterDemo" --check-project "$repo\codewhale-desktop\src-tauri\resources\godot-check" --out $out --ollama-url 'http://127.0.0.1:11434' --model $p[0] --timeout 1500 --base-config "$bench\config-$($p[1]).toml" | Out-File -LiteralPath "$out.log" -Encoding UTF8
    }
}
```

```powershell
$bench = 'D:\AI\cw-bench\qwen-max'
Get-ChildItem -LiteralPath "$bench\a" -Directory | ForEach-Object {
    $r = Get-Content -LiteralPath (Join-Path $_.FullName 'report.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $log = [string](Get-Content -LiteralPath (Join-Path $_.FullName 'codewhale.log') -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
    [pscustomobject]@{
        run = $_.Name; pass = $r.ok; minutes = [math]::Round($r.summary.turn_duration_ms / 60000, 1)
        requests = $r.summary.model_requests; failed_tools = $r.summary.tool_calls_failed
        compactions = $r.summary.compactions; max_input = $r.summary.input_tokens_max
        window_ok = ($r.steps | Where-Object { $_.step -like 'context window*' }).matches_config
        http500 = [regex]::Matches($log, 'HTTP 500|status[=: ]+500').Count
    }
} | Export-Csv -LiteralPath "$bench\a-summary.csv" -NoTypeInformation -Encoding UTF8
```

Прогон с `window_ok` ≠ `True` не засчитывается.

### 3.6 KV-кэш и `--fit`: env Ollama глобальный, нужен перезапуск

Трей читает env только при старте, поэтому f16-эталон идёт на отдельном `ollama serve` с env
процесса; env пользователя не меняется. Если оболочка убивает дочерние процессы — запускать фоном.

```powershell
Get-Process 'ollama app' -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process ollama -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process llama-server -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$env:LOCALAPPDATA\Programs\Ollama\*" } | Stop-Process -Force
$env:OLLAMA_KV_CACHE_TYPE = 'f16'
Start-Process -FilePath "$env:LOCALAPPDATA\Programs\Ollama\ollama.exe" -ArgumentList 'serve' -WindowStyle Hidden -RedirectStandardError 'D:\AI\cw-bench\qwen-max\serve-f16.log'
Start-Sleep -Seconds 5
Invoke-RestMethod -Uri 'http://127.0.0.1:11434/api/version'
```

Здесь `deepseek-r1-7b-32k`: (c) с `$kv = 'f16'`, (b) с тегами `qm_b_ds1`/`qm_b_ds2`, (a) — один
прогон `lean`. В `serve-f16.log` не должно быть `q8_0`. Опция: так же с q8_0 и
`$env:LLAMA_ARG_FIT_TARGET = '512'` повторить (c) для 9B.

Вернуть трей:

```powershell
Get-Process ollama -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process llama-server -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$env:LOCALAPPDATA\Programs\Ollama\*" } | Stop-Process -Force
Start-Process -FilePath "$env:LOCALAPPDATA\Programs\Ollama\ollama app.exe"
```

### 3.7 Оценка и выбор

- **(a)** PASS = `report.json` `ok: true`: ход завершён, шейдер записан, `godot_check` вызван,
  вердикт `ok`. Метрика — доля PASS на «модель + профиль». При равенстве (±1 прогон) решают
  медиана `minutes`, затем `failed_tools`, затем `http500`.
- **(b)** Шкалы 1–5 из `explain-model-eval.md` § 1 (Верн., Прост., Стр., Схема), вслепую.
  Жёсткие правила: корни 2 и 3 с суммой **5**, иначе Верн. ≤ 2; `fact(3) = 6`; схема не
  рисуется — 1; LaTeX — Прост. −1. Балл — среднее по 10 ответам; разница < 0,3 — шум.
- **(c)** `eval_tok_s` для `short-warm` и `long-warm`, `prefill_tok_s` для `long-cold`,
  100 % GPU, `load_s`.

Выбор:
- **Агент** — максимум PASS при `long-warm` ≥ 20 ток/с. MoE становится основной, только если
  PASS не хуже лучшей плотной модели, первый ход ≤ 60 с и commit с открытым Godot < 28 ГБ.
- **Профиль `lean`** — если PASS не ниже, чем у `base` и `think`.
- **Объясняльщик** — лучший балл (b) при ответе ≤ 30 с; при разнице < 0,3 — более быстрая
  модель.

Итог — в `D:\AI\cw-bench\qwen-max\summary.md`.

## 4. Что может сделать только пользователь

1. **Разрешения:** скачать 35–45 ГБ, перезапускать Ollama, на время замера закрыть Godot и ComfyUI.
2. **Питание:** зарядка, «Максимальная производительность» в Windows, производительный режим в
   Lenovo Vantage / Legion Space (эффект не замерен).
3. **MUX:** гибридный режим — рабочий стол на Radeon 860M, VRAM RTX 5060 свободна (перезагрузка).
4. **Панель NVIDIA:** «CUDA – Sysmem Fallback Policy» → «Prefer No Sysmem Fallback» для `llama-server.exe`.
5. **Env Ollama, только после бенчмарка**, затем перезапустить трей; откат — значение `$null`:
   `[Environment]::SetEnvironmentVariable('LLAMA_ARG_CACHE_RAM', '4096', 'User')`.
   `LLAMA_ARG_FIT_TARGET`: 1536, когда открыт Godot; 512 — только если § 3.6 показал пользу.
6. **ОЗУ для MoE:** подкачка «по выбору системы» (администратор); по желанию 2 × 32 ГБ DDR5-5600
   SODIMM — это объём, а не скорость.
7. **Конфиг CodeWhale (§ 5)** пользователь правит сам после бенчмарка.

## 5. Минимальный конфиг CodeWhale против накладных

Замер: `codewhale-app-server.exe` 0.10.1 против `mock_ollama.py`, временный home, копия конфига,
тело первого `/v1/chat/completions`. Токены — по калибровке 47,0 тыс. символов = 10 275 токенов
(≈ 0,219 на символ, ±10 %).

| Конфигурация | Инстр. | Схемы, симв. | System, симв. | ≈ токенов |
| --- | ---: | ---: | ---: | ---: |
| Стандартные `[features]` (прогоны 16–17.10) | 13 | 37 277 | 9 114 | 10 275 (замер) |
| `code_mode` и `subagents` = false | 10 | 12 041 | 9 894 | ≈ 4 900 |
| + `[tools] always_load` из 3 MCP-инструментов (в конфиге с 23:13 19.10) | 13 | ≈ 13 770 | 9 894 | ≈ 5 300 |
| + goal-инструменты отключены | 10 | ≈ 9 080 | 9 894 | ≈ 4 250 |

Ещё из замера:
- 55 навыков промпт не раздувают: индекс обрезается.
- Конфигом не убрать «конституцию» (≈ 4k символов), блок MCP Registry (1,6k) и схему `bash` (2,6k).
- В Ollama уходит `think: true` при `"auto"`/`"high"` и `false` при `"none"`, `max_tokens` 8192,
  без `temperature` и `top_p`.

Дописать в конец `config.toml` (подтаблица уже существующей `[tools]`):

```toml
[tools.overrides]
"create_goal" = { type = "disabled" }
"get_goal" = { type = "disabled" }
"update_goal" = { type = "disabled" }
```

Если `lean` не хуже — `reasoning_effort = "none"` и окна новых вариантов (с `:latest` и без).
`todo_write` оставить (≈ 170 токенов).

## 6. Источники

- https://ollama.com/library/qwen3.5/tags, …/qwen3.6/tags, …/qwen3.8/tags (2026-10-19)
- https://huggingface.co/api/models/unsloth/Qwen3.6-35B-A3B-GGUF/tree/main (и `Qwen3.5-9B-GGUF`, `Qwen3.8-27B-GGUF`, `t-tech/T-lite-it-2.1-GGUF`)
- https://huggingface.co/Qwen/Qwen3.5-9B, https://huggingface.co/Qwen/Qwen3.6-35B-A3B, https://huggingface.co/t-tech/T-lite-it-2.1
- https://artificialanalysis.ai/articles/qwen3-5-small-models, https://artificialanalysis.ai/models/comparisons/qwen3-6-35b-a3b-vs-qwen3-5-9b
- https://github.com/ollama/ollama/blob/v0.40.4/llm/llama_server.go, …/openai/openai.go; issues #16383, #18563
- https://raw.githubusercontent.com/ggml-org/llama.cpp/b11521/common/fit.cpp
- https://raw.githubusercontent.com/codewhale-hq/CodeWhale/v0.10.1/config.example.toml
- https://dev.to/plasmon_imp/moe-beat-dense-27b-by-24x-on-8gb-vram-the-35b-a3b-benchmark-nobody-expected-24n
- Локально: `server.log` Ollama, `D:\AI\cw-bench\final-live-qwen3-4b-run2\events.jsonl`, `D:\AI\cw-bench\final-mock-wave1-baseconfig\ollama_requests.jsonl`, `explain-model-eval.md`
