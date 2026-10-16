# План локальных моделей под RTX 5060 8 ГБ (Windows, 32 ГБ ОЗУ)

Синтез по материалам `docs/research/` (research-items.json, research-now.json → `ollama-tuning`,
user-pdf-notes.md, understand-ollama-api.md), по знаниям о моделях, уже зашитым в код
(`ModelManager.tsx`, `models.rs`, `drive.rs`, `comfy.rs`), и по `docs/FINAL_TEST_WATER.md`,
`docs/KNOWLEDGE_DRIVE.md`, `docs/COMFYUI.md`. Дата: 2026-10-16.

Железо (`docs/research/user-hardware.md`): NVIDIA GeForce RTX 5060 8 ГБ (Blackwell, compute
capability 12.x), 32 ГБ ОЗУ, Windows 10/11, PowerShell 5.1, рядом работает редактор Godot 4.8;
раньше использовались `deepseek` (локально) и `qwen3-4b-32k`.

Три оговорки ко всему документу.
- **Теги и размеры моделей дрейфуют.** Все теги взяты из research JSON или из
  `RECOMMENDED_MODELS` (`codewhale-desktop/src/components/models/ModelManager.tsx:59-81`),
  размеры — это размер скачивания тега по умолчанию (Q4_K_M / MXFP4) и «may drift over time»
  (там же, строка 44). Перед скачиванием сверяйтесь с `ollama show <model>` и ollama.com/library.
- **Цифры VRAM — расчёт по формуле из `models.rs`, а не замер.** В этой сессии не было ни GPU,
  ни доступа к huggingface.co / ollama.com, поэтому параметры архитектур Qwen/Gemma (число
  слоёв, KV-голов) не проверены; где они нужны, используется опорный профиль из комментария
  в коде (см. § 2) и это помечено. Окончательную цифру даёт оценка в менеджере моделей
  (`models_estimate_vram`, `models.rs:746-764`) и `ollama ps` после загрузки.
- Ни одна модель на реальной машине пользователя в рамках этой работы не прогонялась; финальный
  тест воды (§ 3) шёл на сценарном `mock_ollama.py`.

## 1. Кратко

Порог «влезает с запасом» — правило приложения: `total <= gpuTotal * 0.85`
(`ModelManager.tsx:129`, «Leaves ~15% of VRAM for Windows, the browser and other apps»), то есть
≈ 6,8 GiB из 8 GiB. До 8,0 GiB — «впритык», выше — «не влезает» (часть слоёв уедет в ОЗУ).

| Роль | Тег | Квант | `num_ctx` | VRAM при этом ctx: KV f16 / KV q8_0 | Влезает в 8 ГБ с запасом ~15 %? | Примечание |
| --- | --- | --- | --- | --- | --- | --- |
| Главный агент (инструменты, правки, план) | `qwen3.5:9b` (тег из research [9]; сверить) | Q4_K_M, ≈ 5,5 ГБ (research [9]) | 32768 | плотный профиль (верхняя граница): 10,0 / 8,1 GiB; гибридный профиль (как Qwen3.6-35B-A3B, 20 KiB/ток): 6,6 / 6,3 GiB | **зависит от KV-профиля — не проверено**; с q8_0 и гибридным профилем — да | Лучшие вендорские числа по tool-use в классе ≤ 9B (BFCL-V4 66.1, TAU2 79.1), независимые индексы ниже; гибридная архитектура: `OLLAMA_NUM_PARALLEL` принудительно 1, кэш откатывается только к чекпоинтам; обязателен тест на вызов инструментов (§ 6) |
| Быстрый кодер (проверенный запасной) | `qwen2.5-coder:7b` | Q4_K_M, ≈ 4,7 ГБ (`ModelManager.tsx:71`, `drive.rs:617-621`) | 16384 (с q8_0 — 24576) | 7,2 / 6,3 GiB (при 24576: 8,2 / 6,8) | f16 — впритык; **q8_0 — да** | Рекомендован в `FINAL_TEST_WATER.md` для повторения теста на Windows (`num_ctx` ≥ 16k); «золотой набор» `drive.rs`; родной контекст всего 32k |
| Рассуждения / план | та же `qwen3.5:9b` с `reasoning_effort` high; запасной — `qwen3:8b` | Q4_K_M, ≈ 5,2 ГБ (`ModelManager.tsx:73`) | 16384 | `qwen3:8b`: 7,7 / 6,8 GiB | q8_0 — на грани (6,8) | Отдельная «модель-архитектор» на 8 ГБ не живёт рядом с кодером: каждое переключение — выгрузка, загрузка и полный re-prefill. Дешевле один агент и уровень thinking по шагу (research [25]) |
| Эмбеддинги (для `vectordb`, «позже» по `KNOWLEDGE_DRIVE.md`) | `bge-m3` | ≈ 1,2 ГБ (`drive.rs:647-651`) | — (эмбеддер) | ≈ 1,7 GiB (веса + 0,5 GiB служебных; KV-кэш генерации не нужен) | да, но не одновременно с главной моделью на длинном контексте | Загружать только на время построения индекса |
| Зрение (опционально) | `gemma4:e4b` | размер тега **не проверен** (в research нет цифры) | 16384 | не посчитано — нет параметров архитектуры | не проверено | 128k контекст, нативный function calling (research [7]); роль «объяснить/прочитать скриншот», не главный агент без smoke-теста |
| Профиль «Godot открыт» (мало VRAM) | `qwen3:4b-instruct-2507-q4_K_M` (семейство вашей `qwen3-4b-32k`) | Q4_K_M, ≈ 2,5 ГБ (`ModelManager.tsx:62-69`) | 32768 | 7,0 / 5,1 GiB | f16 — впритык; **q8_0 — да, и остаётся ≈ 1,7 GiB под редактор** | «Лучший баланс для 6–8 ГБ» по заметке каталога; research [13] считает её устаревшей рядом с `qwen3.5:9b`, но как запасной вариант при занятой VRAM она самая безопасная |

Итог в одну строку: `OLLAMA_KV_CACHE_TYPE=q8_0`, один главный агент (`qwen3.5:9b`, если
пройдёт § 6; иначе `qwen2.5-coder:7b`) в варианте с запечённым `num_ctx` 16–32k, `qwen3:4b`
как профиль для работы с открытым Godot, остальное — по требованию.

## 2. Бюджет видеопамяти

### Кто ещё живёт в 8 ГБ

| Потребитель | Сколько | Откуда цифра |
| --- | --- | --- |
| Windows DWM, браузер, трей-приложения | ≈ 0,5–1 ГБ | ориентир из постановки задачи; измерить `nvidia-smi` на «пустом» столе |
| Редактор Godot 4.8 | **не проверено** | измерить: `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv` при открытом проекте (research [23] предлагает именно это) |
| ComfyUI во время рендера | чекпоинт 6–8 ГБ (`docs/COMFYUI.md`), плюс `--reserve-vram 1.0` (`comfy.rs:246`) | с моделью Ollama не совмещается, поэтому есть «эстафета VRAM» (§ 5) |
| Запас приложения | 15 % ≈ 1,2 GiB | `ModelManager.tsx:129` |

Рабочий бюджет под модель: ≈ 6,8 GiB без Godot; с открытым редактором — 6,8 GiB минус его
замеренное потребление (ориентировочно 5 GiB и меньше, пока не замерено).

### Формула оценки (из `models.rs:650-666`)

```
total    = weights + KV + RUNTIME_OVERHEAD_BYTES              # RUNTIME_OVERHEAD_BYTES = 512 MiB (models.rs:36)
weights  = size из /api/tags (включая vision-проектор)
KV       = num_ctx * sum_по_слоям( n_head_kv(l) * (key_length + value_length) ) * bytes_per_element
           key_length / value_length по умолчанию = embedding_length / head_count
           bytes_per_element: f16 = 2, q8_0 = 34/32, q4_0 = 18/32   (models.rs:669-671)
           num_ctx обрезается до обучающего контекста модели
```

Реализация: `kv_elements_per_token` (`models.rs:674-689`) суммирует KV-головы по слоям (для
гибридных моделей `attention.head_count_kv` — массив, 0 = слой без внимания), `kv_cache_bytes`
(`models.rs:691-694`) умножает на `num_ctx` и байты на элемент, `estimate_vram`
(`models.rs:717-742`) складывает три слагаемых и отдаёт итоги для f16 / q8_0 / q4_0. Формула —
верхняя граница для sliding-window-моделей (Gemma 3, gpt-oss) и MLA (DeepSeek) и предполагает
`OLLAMA_NUM_PARALLEL=1`: каждый параллельный слот добавляет полный `num_ctx` KV-кэша
(`models.rs:664-666`).

Опорный профиль, которым здесь считаются плотные 7–9B модели (его параметры — единственные,
которые подтверждены исходником): пример из комментария `models.rs:661-662` — Llama 3 8B,
32 слоя × 8 KV-голов × head_dim 128 → `32 × 8 × (128 + 128) × 2 Б = 128 KiB` на токен в f16.
У Qwen2.5-7B / Qwen3-8B KV-голов, по карточкам моделей, меньше (не проверено в этой сессии),
то есть реальные цифры ниже опорных; у гибридного семейства Qwen3.5/3.6 — ниже в разы
(у Qwen3.6-35B-A3B: 40 слоёв, из них 10 с полным вниманием, 2 KV-головы, head_dim 256 →
20 KiB/ток, research [1]).

### Два разобранных примера

**Пример A — плотная 8B (`llama3.1:8b`, 4,9 ГБ по `ModelManager.tsx:72`), профиль выше.**
- `num_ctx = 16384`: KV f16 = 16384 × 65536 эл. × 2 Б = 2,0 GiB; q8_0 = ×(34/32)/2 = 1,06 GiB.
- total f16 = 4,9 + 2,0 + 0,5 = **7,4 GiB** → впритык (> 6,8, ≤ 8).
- total q8_0 = 4,9 + 1,06 + 0,5 = **6,46 GiB** → влезает с запасом.
- `num_ctx = 32768`: f16 9,4 GiB (не влезает), q8_0 7,53 GiB (впритык).

**Пример B — MoE `qwen3-coder:30b` (19 ГБ по `ModelManager.tsx:80`), 96 KiB/ток в f16
(48 слоёв × 4 KV-головы × head_dim 128, research [1]/[13]).**
- `num_ctx = 32768`: KV f16 = 3,0 GiB; q8_0 = 1,6 GiB.
- total q8_0 = 19 + 1,6 + 0,5 = **21,1 GiB** — в 8 ГБ не помещается в принципе: на GPU
  останутся KV-кэш и ≈ 3–4 GiB весов, остальное — в ОЗУ. Что именно уедет в ОЗУ — целые слои
  или только тензоры экспертов — зависит от версии Ollama (§ 8, вопрос 2); в обоих случаях
  `ollama ps` покажет «N%/M% CPU/GPU», а не «100% GPU».

### Ловушка контекста по умолчанию

Ollama выбирает `num_ctx` по объёму VRAM: < 24 GiB → 4096, 24–48 → 32768, ≥ 48 → 262144;
в коде пороги 23 / 47 GiB после вычитания `OLLAMA_GPU_OVERHEAD` (research [14], [2];
в приложении — `ollamaDefaultCtx`, `ModelManager.tsx:95-99`). На 8 ГБ это **4k**, а
документация Ollama говорит, что агентам и coding-инструментам нужно «не меньше 64000».
CodeWhale ходит в Ollama через OpenAI-совместимый `/v1`, который **не умеет передавать
`num_ctx`** (research [2]; `understand-ollama-api.md` § 0). Переполнение Ollama по умолчанию
режет историю молча (`truncate`/`shift` = true, в `/v1` их нельзя выключить; research [71]),
и в исследовании Aider это стоило модели 71,4 % → 51,9 % решённых задач (research [29]).
Значит, `num_ctx` надо запечь: вариантом модели (`ollama_create_variant`, `ollama.rs:131-150`,
диапазон 2048..262144) или глобально через `OLLAMA_CONTEXT_LENGTH` (§ 4). Менять `num_ctx` от
запроса к запросу нельзя — это полная перезагрузка модели (research [29]).

## 3. Кандидаты по ролям

Что говорит опыт проекта:
- `qwen3-4b-32k` (вариант `qwen3:4b` с запечённым 32k) — рабочая модель пользователя;
  `PREFERRED_MODELS` в `useBootstrap.ts` до сих пор ищет её по имени (`understand-ollama-api.md`
  B2: без суффикса `:latest` она не находится).
- `deepseek` локально — какой именно тег, не зафиксировано; research [13] относит дистилляты
  `deepseek-r1:*` к плохим «вызывателям инструментов» (сообщения «does not support tools»),
  а актуальные DeepSeek V4 (284B/13B active и больше) локально не запускаются.
- Финальный тест воды (`docs/FINAL_TEST_WATER.md`): цепочка CodeWhale → `tool_search` →
  `mcp_ai-drive_godot_check` → Godot headless прошла, но текст модели давал `mock_ollama.py`
  (сценарий `[shader]`). **Ни одна настоящая модель тест не проходила.** Для повторения на
  Windows документ называет `qwen2.5-coder:7b` с `num_ctx` ≥ 16k.

### Главный агент

| Кандидат | За | Против |
| --- | --- | --- |
| `qwen3.5:9b` (research [9]) | самые сильные вендорские tool-use числа в 8–12 ГБ классе; ≈ 5,5 ГБ Q4_K_M; гибрид → маленький KV-кэш; есть MTP-головы для спекулятивного ускорения (`draft_num_predict`, research [17], beta); research [13] предлагает именно её как замену `deepseek-r1:*` и `qwen3:4b` | независимые индексы оценивают агентность ниже вендора; семейство Qwen3.x на Ollama страдает от строгого XML-парсера tool-calls — длинные записи файлов (цельный `.gdshader`) могут упасть с ошибкой парсера как «ответ модели» (research [4]); гибрид: компакция истории = re-prefill от последнего чекпоинта (research [18]) |
| `qwen2.5-coder:7b` | специализирован на коде; 4,7 ГБ; проверенный тег в «золотом наборе» и в `FINAL_TEST_WATER.md`; плотная модель без особенностей гибридов | родной контекст 32k; старее на поколение; по research [9] класс 7B-кодеров «уже заменён» |
| `ministral-3:8b` (research [9]) | нативный tool use, 256k контекст, отдельный парсер `ministral` в Ollama; Mistral советует temperature < 0,1 и маленький набор инструментов | ≈ 6,0 ГБ скачивания → даже при 8k контекста 7,0–7,5 GiB по опорному профилю; на 8 ГБ полезного контекста почти не остаётся |

### Быстрый кодер (режим Work, короткие правки)

Та же `qwen2.5-coder:7b` в варианте 16–24k, либо `qwen3:4b-instruct-2507-q4_K_M` (2,5 ГБ,
262k родного контекста, «уверенно вызывает инструменты» по заметке каталога) — последняя
остаётся единственным кандидатом, когда Godot занял 2–3 ГБ. `qwen2.5-coder:1.5b` из «золотого
набора» (`drive.rs:641-645`, «черновик / автодополнение») на этой карте смысла почти не имеет:
отдельная draft-модель занимает VRAM, а у Qwen3.5 ускорение дают встроенные MTP-головы.

### Рассуждения / план

Отдельная рассуждающая модель (в `drive.rs:635-639` это `deepseek-r1:14b`, 9 ГБ) на 8 ГБ
не влезает и сама по себе, и рядом с кодером. Практичнее:
1. тот же `qwen3.5:9b` с уровнем thinking «high» на шаге планирования и «low/off» на правках —
   Ollama принимает `reasoning_effort` через `/v1`, а `/api/show` с v0.34.3 сообщает уровни
   thinking модели (research [25]);
2. запасной — `qwen3:8b` (5,2 ГБ, thinking + tools, 40k контекста).
PDF-схема Plan → Work с двумя моделями здесь превращается в «одна модель, два уровня thinking».

### Эмбеддинги

`bge-m3` (≈ 1,2 ГБ) — для LanceDB/семантического поиска, который в `KNOWLEDGE_DRIVE.md`
помечен «позже»; индекс `git_vault` сейчас FTS5 и эмбеддингов не требует. Загружать на время
индексации и выгружать (вкладка «Запущенные»). Будет ли Ollama держать его рядом с главной
моделью или выгрузит её — не проверено (§ 8).

### Зрение (опционально)

`gemma4:e4b` — чтение скриншотов артефактов шейдера; размер тега не зафиксирован в research,
агентность в чужих таблицах ниже Qwen3.6 (research [7]). `mistral-small3.2:24b` (15 ГБ, vision +
tools, `ModelManager.tsx:79`) на эту карту не рассчитан.

### Растяжка: MoE с экспертами в ОЗУ

`qwen3:30b-a3b` из «золотого набора» (`drive.rs:623-627`, 19 ГБ), `qwen3-coder:30b` (19 ГБ),
`qwen3.6:35b-a3b` (22–24 ГБ Q4_K_M, research [3]). При 3B активных параметров они могут быть
«заметно умнее» при приемлемой скорости — но только если тензоры экспертов уедут в ОЗУ, а
внимание и KV останутся на GPU. Research [16] утверждает, что Ollama ≥ 0.30 через llama.cpp
`--fit` делает это сама («all MoE tensors in system memory»), research [3] — что Ollama
выгружает целые слои и per-model `n-cpu-moe` нет (issue #11772). Единственный близкий замер —
llama-server напрямую: RTX 2060 6 ГБ, Qwen3.5-35B-A3B Q4_K_M, `--n-cpu-moe 99`, KV q8_0,
контекст 102k → ≈ 15 ток/с генерации, 350–400 ток/с prefill (research [16]). Для 8 ГБ + 32 ГБ
ОЗУ это эксперимент после базовой настройки, не повседневная модель; веса 22–24 ГБ в ОЗУ
рядом с Godot и браузером — на грани 32 ГБ (не проверено).

## 4. Настройка Ollama на Windows

Все переменные — серверные, читаются при старте; на Windows трей-приложение подхватывает их
только после выхода и повторного запуска (research [19], docs/windows.mdx).

| Переменная | Значение | Зачем | Источник |
| --- | --- | --- | --- |
| `OLLAMA_KV_CACHE_TYPE` | `q8_0` | KV-кэш вдвое меньше f16; после Hadamard-поворота в llama.cpp (апрель 2026) почти без потерь: Qwen3.5-9B KLD 0.000782 (f16) против 0.000799 (q8_0). **Не `q4_0`** для агентной работы — llama.cpp предупреждает о деградации tool calling | research [15], [29] |
| `OLLAMA_FLASH_ATTENTION` | не задавать (или `1`) | включается автоматически, где поддерживается; `q8_0` требует flash attention | research [2], [15] |
| `OLLAMA_KEEP_ALIVE` | `30m` (PDF) … `2h` (research [18]) | выгрузка через 5 мин по умолчанию выбрасывает весь KV-кэш → полный re-prefill длинной истории; перед ComfyUI модель выгружается явно (§ 5) | research [18], [71] |
| `OLLAMA_NUM_PARALLEL` | `1` | и так по умолчанию 1; для семейства qwen35 Ollama принудительно ставит 1; память растёт как PARALLEL × CONTEXT. В CodeWhale поставить `[subagents] max_concurrent = 1`, чтобы суб-агенты не вставали в очередь Ollama | research [22] |
| `OLLAMA_GPU_OVERHEAD` | байты, например `1073741824` (1 GiB) при открытом Godot | «Reserve a portion of VRAM per GPU (bytes)» — имя подтверждено в envconfig; в PDF названо `OLLAMA_VRAM_OVERHEAD` — такой переменной нет | research [19], [23] |
| `LLAMA_ARG_FIT_TARGET` | MiB, по умолчанию 1024 | запас свободной VRAM для авторазмещения llama.cpp внутри Ollama ≥ 0.30; research [16] советует 2048 при запущенном Godot. Какая из двух переменных «главнее» — не проверено | research [16], [19] |
| `OLLAMA_CONTEXT_LENGTH` | например `32768` | глобальная альтернатива вариантам; слайдер контекста в приложении Ollama пишет именно её. Минус: действует на все модели (и на 128k-модели, которым это не по размеру) | `understand-ollama-api.md` B4, F10 |
| `OLLAMA_MODELS` | `<диск>\models` | кнопка **OLLAMA_MODELS → диск** ставит пользовательскую переменную через `[Environment]::SetEnvironmentVariable(…, 'User')` (`drive.rs:312-327`), **Перезапустить Ollama** делает `taskkill` и запускает `ollama app.exe` (`drive.rs:330-360`). Внешний диск = более долгая загрузка после каждой выгрузки; веса остаются в standby-кэше ОЗУ (`docs/COMFYUI.md`) | `docs/KNOWLEDGE_DRIVE.md` |
| `OLLAMA_NO_CLOUD` | `1` (по желанию) | облачные стабы в `/api/tags` выглядят как локальные модели | `understand-ollama-api.md` B5 |

Где задать (PowerShell 5.1, без `&&`):

```powershell
[Environment]::SetEnvironmentVariable('OLLAMA_KV_CACHE_TYPE', 'q8_0', 'User')
[Environment]::SetEnvironmentVariable('OLLAMA_KEEP_ALIVE', '30m', 'User')
[Environment]::SetEnvironmentVariable('OLLAMA_NUM_PARALLEL', '1', 'User')
# затем выйти из трея Ollama и запустить заново (или кнопка «Перезапустить Ollama» в панели «Диск знаний»)
```

Проверка, что сервер увидел переменные: в `%LOCALAPPDATA%\Ollama\server.log` строка
`server config env=map[…]` перечисляет `OLLAMA_KV_CACHE_TYPE`, `OLLAMA_FLASH_ATTENTION`,
`OLLAMA_CONTEXT_LENGTH`, `OLLAMA_MODELS`, а строка `vram-based default context … default_num_ctx=N`
показывает выбранный по VRAM контекст (`understand-ollama-api.md` F9).

NVIDIA: панель управления → «CUDA – Sysmem Fallback Policy» → **Prefer No Sysmem Fallback**
(драйвер ≥ 546.01). Иначе при нехватке VRAM драйвер молча уводит CUDA-аллокации в системную
память и ток/с проваливается без ошибки; с запретом — честное падение, которое видно
(research [23]). Ollama на Windows требует драйвер ≥ 551.61 (там же). Blackwell: в llama.cpp
CUDA есть NVFP4/MXFP4 через FP4-тензорные ядра (research [24]) — при выборе между тегами
предпочитать NVFP4/MXFP4-сборки, если они появятся в библиотеке для нужной модели (не проверено).

Вариант с запечённым `num_ctx`:
- в приложении: «Модели» → «Установленные» → **Вариант с num_ctx** (`ModelManager.tsx:481`),
  варианты делят блобы с базовой моделью и не занимают диск (`ollama.rs:131`); имя без тега
  появится как `…:latest` (`understand-ollama-api.md` B2);
- вручную, Modelfile:

```powershell
Set-Content -Path .\Modelfile -Value "FROM qwen3.5:9b`nPARAMETER num_ctx 32768"
ollama create qwen3.5-9b-cw32k -f .\Modelfile
ollama show qwen3.5-9b-cw32k
```

Затем сказать CodeWhale реальное окно, иначе его компакция (по умолчанию 80 % окна,
research [14]) никогда не сработает, а Ollama будет резать молча:
`~/.codewhale/config.toml` → `[providers.ollama] model_context_windows = {"qwen3.5-9b-cw32k:latest" = 32768}`
(`understand-ollama-api.md` F11; доходит ли это до предупреждения в Inspector — не проверено).

Проверка загрузки: `ollama ps` → колонка PROCESSOR `100% GPU` и CONTEXT = запечённому
значению (research [2], [71]); в приложении — вкладка «Запущенные» (`ModelManager.tsx:321`)
и Inspector «В видеопамяти … из …» с предупреждением, когда `size_vram < size`
(`Inspector.tsx:195`).

## 5. Сосуществование с Godot и ComfyUI

1. **ComfyUI — автоматическая эстафета.** Перед каждым рендером MCP-сервер `comfy` берёт
   `GET /api/ps` и выгружает каждую модель Ollama (`POST /api/generate {model, keep_alive: 0}`,
   `comfy_mcp.rs:307-317`), после рендера — `POST /free {"unload_models": true, "free_memory": true}`
   (`comfy_mcp.rs:319-322`). Отключается `COMFY_UNLOAD_LLM=0` / настройка `comfy.unloadLlm`
   (`comfy.rs:89`, `comfy.rs:127-129`); кнопка `comfy_free` делает то же вручную
   (`comfy.rs:372-385`). ComfyUI стартует с `--lowvram --reserve-vram 1.0` (`comfy.rs:246`).
   После рендера первая реплика агента платит за повторную загрузку (с внешнего диска дольше)
   и за полный re-prefill истории.
2. **Godot — вручную.** Редактор открывают до загрузки модели (тогда Ollama сама увидит меньше
   свободной VRAM) либо задают `OLLAMA_GPU_OVERHEAD` / `LLAMA_ARG_FIT_TARGET` под его замеренное
   потребление. Перед запуском тяжёлой сцены (F5) — «Выгрузить» во вкладке «Запущенные» или
   дождаться `keep_alive`. Headless-проверки `godot_check` (`docs/KNOWLEDGE_DRIVE.md`) VRAM
   практически не трогают (не измерено).
3. **Резидентной держать одну модель** — главного агента в варианте с `num_ctx`. Эмбеддер и
   vision-модель — по требованию, с выгрузкой после.
4. **Профиль «Godot открыт»**: переключиться на `qwen3:4b-instruct-2507` 32k (5,1 GiB с q8_0),
   принять, что переключение = перезагрузка.
5. Признак беды: в «Запущенные» `size_vram < size`, в `ollama ps` не `100% GPU`, ток/с упал
   в разы, в Диспетчере задач растёт «Общая память GPU» при упёршейся «Выделенной» — это
   sysmem fallback или выгрузка слоёв (research [23], [71]).

## 6. Проверка

Чек-лист приёмки модели (минимум — для `qwen3.5:9b` и `qwen2.5-coder:7b`):

1. **100 % GPU.** `ollama ps` после первого запроса: `100% GPU`, CONTEXT = `num_ctx` варианта.
   В приложении: «Запущенные» без предупреждения о частичной загрузке.
2. **Контекст ≥ 16k.** `ollama show <вариант>` показывает `num_ctx`; в StatusBar приложения
   окно (`window_tokens`) совпадает с ним, а не с «4k по умолчанию» (`StatusBar.tsx:54-69`,
   хардкод «4k» — известный баг B4 в `understand-ollama-api.md`).
3. **Вызов инструментов.** Короткий сценарий через настоящий CodeWhale: `tool_search ai-drive`
   → `godot_check` на заведомо сломанном `.gdshader` → правка → повтор (это и есть сценарий
   `FINAL_TEST_WATER.md`, § «Как повторить у себя»). Отдельно — запись файла на 300–400 строк
   целиком: ловит падение XML-парсера Qwen (research [4]). Если упало — скилл/подсказка «писать
   кусками ≤ 200 строк или через patch».
4. **Скорость — здравый смысл, а не абсолют.** Эталонных ток/с для RTX 5060 8 ГБ в research
   нет; зафиксируйте свой baseline (`ollama run <model> --verbose` → `eval rate`) и следите, чтобы
   он не проседал в разы при росте контекста. Цифра «ток/с» в StatusBar сейчас занижена: она
   делит выход на всё время хода, включая prefill и загрузку (`threads.ts:272`, research [70]).
5. **Прогрев.** `POST /api/generate {"model": "<вариант>", "keep_alive": "30m"}` без `num_ctx`
   в options (иначе лишняя перезагрузка; `understand-ollama-api.md` F6).
6. **Журнал.** `OLLAMA_DEBUG=1` + поиск `truncating input messages` в `server.log` после долгой
   сессии — если есть, окно CodeWhale больше реального (research [71]).

Что из этого уже показывает приложение: вкладки «Установленные» (оценка VRAM f16/q8_0 на
выбранном контексте, `ModelManager.tsx:589-590`), «Запущенные» (`size_vram`/`size`), Inspector
(контекст и видеопамять), StatusBar (использование окна, ток/с). Чего нет: smoke-теста на
инструменты и бенчмарка по размерам контекста (research [10], [77] — предложения).

## 7. Расхождения с PDF пользователя

| PDF (`user-pdf-notes.md`, «Inference / VRAM») | Что говорят источники |
| --- | --- |
| `OLLAMA_VRAM_OVERHEAD` для резерва под Godot/браузер | переменной нет; есть `OLLAMA_GPU_OVERHEAD` (байты) и `LLAMA_ARG_FIT_TARGET` (MiB) — research [19], [23] |
| `OLLAMA_FLASH_ATTENTION=1` | уже автоматически, где поддерживается; задавать не вредно — research [2], [15] |
| `OLLAMA_KV_CACHE_TYPE=q8_0` | совпадает; добавить «не q4_0 для агентов» и оговорку: DeepSeek-семейство с q8_0 K-кэшем давало мусор (issue #25382) → для deepseek вернуть f16 — research [15] |
| `OLLAMA_KEEP_ALIVE=30m`, `OLLAMA_NUM_PARALLEL=1` | совпадает; research [18] предпочитает 2h/-1 с кнопкой выгрузки, потому что выгрузка = потеря KV-кэша |
| `num_thread` = физические ядра; Q4_K_M/Q5_K_M для частей на CPU | актуально только при выгрузке в ОЗУ; цель плана — держать всё на GPU. Эффект на этой машине не проверен |
| MoE 30B-A3B с экспертами на CPU через llama-server `--override-tensor "\.ffn_.*_exps\.=CPU"` / `--cpu-moe`, «32–48 ток/с (?)» | флаги существуют (`--cpu-moe`, `--n-cpu-moe N`, research [11]); цифры 32–48 не подтверждены — ближайший замер 15 ток/с на 6 ГБ карте (research [16]); в Ollama per-model переключателя нет, но ≥ 0.30 `--fit` может делать это сам (противоречие research [3] и [16], § 8) |
| Авто-балансировщик VRAM↔RAM, считающий `num_gpu` по `nvidia-smi` | не нужен и вреден: явный `num_gpu` отключает авторазмещение `--fit` (research [16]); у приложения уже есть оценка `models_estimate_vram` |
| Prefix caching через llama-server `--cache-reuse` | у Ollama свой кэш: с v0.30.8 отвязан от context shift, с v0.33.3 отдаёт `cached_tokens`; `--cache-reuse` — только для llama-server (research [18]) |
| Спекулятивное декодирование с маленькой draft-моделью (?) | на 8 ГБ отдельная draft-модель занимает VRAM; для Qwen3.5 есть встроенный MTP (`draft_num_predict`, beta; +≈ 10 % VRAM по оценке research [17]), а n-gram-спекуляция без draft-модели — только в llama-server (research [21]) |
| Plan (рассуждающая) → Work (быстрый кодер), две модели | на 8 ГБ одновременно не живут; каждое переключение — перезагрузка и re-prefill. Замена: один агент + уровни thinking (research [25]) |
| Modelfile `num_ctx 24576`, `temperature 0.15` | 24576 есть в `CTX_CHOICES` (`ModelManager.tsx:84`) и влезает для 7B с q8_0; низкая температура совпадает с советом Mistral для Ministral; для Qwen3.5 рекомендованные сэмплеры не проверены |
| «Всегда запекать `num_ctx`» | совпадает полностью (§ 2) |

## 8. Не проверено / открытые вопросы

1. **KV-профиль `qwen3.5:9b`** (`block_count`, `head_count_kv`, доля слоёв с полным вниманием).
   От него зависит, влезает ли 32k: по плотной верхней границе — нет, по гибридной — да.
   Проверить: `ollama show --verbose qwen3.5:9b` → `model_info`, затем оценка в менеджере.
2. **Поведение Ollama ≥ 0.30 с MoE на 8 ГБ**: выгружает тензоры экспертов (research [16]) или
   целые слои (research [3])? Проверить на `qwen3:30b-a3b`: `ollama ps` (доля GPU) и ток/с.
3. **Версия Ollama у пользователя** не зафиксирована. Нужно ≥ 0.30 для llama.cpp-движка,
   ≥ 0.34.3 для уровней thinking в `/api/show`, актуальная стабильная — v0.40.3 (2026-10-11);
   откат ниже 0.40 требует повторного скачивания моделей (research [19]).
4. **Потребление VRAM редактором Godot 4.8** и `nvidia-smi memory.total` для RTX 5060 (здесь
   принято 8192 MiB; реальное значение чуть меньше).
5. **Размер и тег `gemma4:e4b`**, а также NVFP4/MXFP4-теги для Qwen3.5 в библиотеке Ollama.
6. **Сосуществование двух моделей** (`bge-m3` + главная) — как планировщик Ollama поступит,
   когда вторая не влезает; `OLLAMA_MAX_LOADED_MODELS` на 8 ГБ.
7. **`OLLAMA_GPU_OVERHEAD` против `LLAMA_ARG_FIT_TARGET`** — какая из них реально управляет
   запасом в текущей версии и не конфликтуют ли они.
8. **Доходит ли `model_context_windows` CodeWhale до предупреждения Inspector** (F11).
9. **MTP-вариант `qwen3.5:9b`** (`draft_num_predict 2`): прирост и расход VRAM на этой карте;
   имя ключа `nextn_predict_layers` в `model_info` целевой сборки.
10. **Сэмплеры** (temperature, top_p) для Qwen3.5 в агентном режиме.

## 9. Источники

Локальные:
- `docs/research/user-hardware.md`, `docs/research/user-pdf-notes.md`,
  `docs/research/understand-ollama-api.md` (разделы 0, B2, B4, B5, F6, F9, F10, F11).
- `docs/research/research-items.json`: [1] VRAM fit estimator; [2] Ollama context and KV-cache
  configuration; [3] Qwen3.6-35B-A3B; [4] Tool-call reliability guardrails; [6] gpt-oss-20b;
  [7] Gemma 4 family; [8] GLM-4.7-Flash; [9] 8-12 GB tier; [10] Benchmark hygiene;
  [11] llama.cpp backend; [13] Legacy and stretch models; [14] Context-window autosizing;
  [15] KV-cache quantization; [16] llama.cpp --fit; [17] MTP speculative decoding;
  [18] Prefix-cache hygiene; [19] Managed Ollama server; [21] n-gram speculation;
  [22] Parallel slots; [23] Windows GPU hygiene; [24] Quantization formats; [25] Thinking budget;
  [29] Context-window autopilot; [70] Correct speed metrics; [71] Context-overflow guard;
  [77] Micro-benchmark. `docs/research/research-now.json` → `ollama-tuning` (те же темы в
  более поздней редакции: контекст, KV, managed server, GPU hygiene, overflow guard).
- Код: `codewhale-desktop/src/components/models/ModelManager.tsx` (44, 59-81, 84, 95-99, 129,
  321, 481, 589-590, 868), `codewhale-desktop/src-tauri/src/models.rs` (36, 650-694, 717-764),
  `codewhale-desktop/src-tauri/src/drive.rs` (312-360, 605-653), `codewhale-desktop/src-tauri/src/comfy.rs`
  (89, 127-129, 246, 372-385), `codewhale-desktop/src-tauri/src/comfy_mcp.rs` (307-322),
  `codewhale-desktop/src-tauri/src/ollama.rs` (131-150), `codewhale-desktop/src/components/layout/Inspector.tsx:195`,
  `codewhale-desktop/src/components/layout/StatusBar.tsx:50-73`, `codewhale-desktop/src/store/threads.ts:272`.
- `docs/FINAL_TEST_WATER.md`, `docs/KNOWLEDGE_DRIVE.md`, `docs/COMFYUI.md`, `docs/HANDOFF_LOCAL_SESSION.md`.

Внешние (как процитированы в research JSON; в этой сессии не перепроверялись):
- https://github.com/ollama/ollama/blob/main/docs/context-length.mdx — VRAM-тиры контекста, «≥ 64000 для агентов».
- https://github.com/ollama/ollama/blob/main/docs/api/openai-compatibility.mdx — `/v1` не передаёт `num_ctx`.
- https://github.com/ollama/ollama/blob/main/docs/faq.mdx — `OLLAMA_KV_CACHE_TYPE`, flash attention, `KEEP_ALIVE`, `NUM_PARALLEL`.
- https://github.com/ollama/ollama/blob/main/envconfig/config.go — `OLLAMA_GPU_OVERHEAD`, `LLAMA_ARG_FIT_TARGET`, `OLLAMA_CONTEXT_LENGTH`.
- https://github.com/ollama/ollama/blob/main/docs/windows.mdx — перезапуск трея, `server.log`, драйвер ≥ 551.61.
- https://github.com/ollama/ollama/blob/main/server/sched.go — принудительный `numParallel = 1` для qwen35 и других гибридов.
- https://github.com/ggml-org/llama.cpp/pull/21038 — Hadamard-поворот, KLD для Qwen3.5-9B.
- https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md — предупреждение о q4_0 KV и tool calling.
- https://github.com/ggml-org/llama.cpp/blob/master/common/fit.cpp, https://github.com/ggml-org/llama.cpp/discussions/21112 — `--fit`, MoE в ОЗУ, замер на 6 ГБ.
- https://github.com/ollama/ollama/issues/11772 — нет per-model `n-cpu-moe` в Ollama.
- https://github.com/ollama/ollama/issues/18563, https://github.com/ollama/ollama/issues/16383 — падения XML-парсера tool-calls Qwen.
- https://huggingface.co/Qwen/Qwen3.5-9B, https://willitrunai.com/blog/qwen-3-5-9b-vram-requirements, https://apxml.com/models/qwen35-9b — Qwen3.5-9B: метрики, ≈ 5,5 ГБ Q4_K_M, независимые оценки.
- https://huggingface.co/Qwen/Qwen3.6-35B-A3B, https://huggingface.co/Qwen/Qwen3-Coder-30B-A3B-Instruct — параметры архитектур для KV-профилей.
- https://huggingface.co/mistralai/Ministral-3-14B-Instruct-2512 — Ministral 3, temperature < 0,1.
- https://blog.google/innovation-and-ai/technology/developers-tools/gemma-4/ — Gemma 4 E4B.
- https://nvidia.custhelp.com/app/answers/detail/a_id/5490 — «CUDA – Sysmem Fallback Policy».
- https://aider.chat/2024/11/21/quantization.html — цена усечённого контекста.
- https://github.com/ollama/ollama/blob/main/docs/modelfile.mdx — `draft_num_predict` (MTP).
