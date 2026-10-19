# Фабрика ассетов: ComfyUI рядом с агентом на 8 ГБ VRAM

Агент CodeWhale получает инструменты генерации игровых ассетов (иконки, PBR-текстуры со
`StandardMaterial3D`, спрайт-листы, 3D-пропы) через MCP-сервер `comfy`, встроенный в
CodeWhale Desktop (`codewhale-desktop.exe mcp-comfy`). Архитектура — «оркестратор шаблонов с
эстафетой памяти» из плана пользователя.

## Две проблемы и как они решены

| Проблема | Решение |
| --- | --- |
| Модель Ollama (~5–7 ГБ) и модель ComfyUI (6–8 ГБ) не помещаются в 8 ГБ VRAM одновременно | **Эстафета VRAM**: перед рендером сервер делает `GET /api/ps` в Ollama и выгружает каждую модель (`POST /api/generate {model, keep_alive: 0}`); после рендера — `POST /free {"unload_models": true, "free_memory": true}` в ComfyUI. Веса остаются в 32 ГБ ОЗУ (standby cache), следующий заход быстрый. |
| Локальная модель не должна сочинять JSON-графы ComfyUI на 40 нод | **Запечённые шаблоны**: граф сохраняется из ComfyUI в API-формате с плейсхолдерами `{{POSITIVE_PROMPT}}`, `"{{SEED}}"`, `"{{WIDTH}}"`…; агент вызывает одну функцию. Все вхождения заменяются, незаполненный плейсхолдер — ошибка. Перед запуском типы нод сверяются с `/object_info`, и ответ называет недостающие кастомные ноды. |

## Настройка (один раз)

1. Установите ComfyUI (портативная сборка или `git clone` + venv) и хотя бы один чекпоинт
   (SD 1.5 / SDXL / Flux GGUF Q4).
2. В приложении: палитра `Ctrl+K` → **Фабрика ассетов**:
   - укажите папку ComfyUI (где `main.py`); интерпретатор подбирается сам (`venv`, `python_embeded`);
   - **Установить шаблоны** — в `<диск>\AI_Workflows` копируются `ui_icon_api.json`,
     `pbr_material_api.json`, `sprite_sheet_api.json` и README, в `<диск>\AI_ComfyModels`
     создаются папки моделей и `extra_model_paths.yaml`, ставится скилл `comfyui-gamedev`;
   - положите чекпоинты в `<диск>\AI_ComfyModels\checkpoints` (или в `models\checkpoints`
     ComfyUI) и выберите его в поле «Чекпоинт» — он подставляется в `{{CHECKPOINT}}`;
   - **Запустить ComfyUI** — приложение запускает
     `python main.py --listen 127.0.0.1 --port 8188 --lowvram --reserve-vram 1.0 --preview-method taesd --extra-model-paths-config <диск>\AI_ComfyModels\extra_model_paths.yaml`
     и ждёт ответа `/system_stats`;
   - **Подключить MCP** — запись `comfy` добавляется в `~/.codewhale/mcp.json`, CodeWhale
     перезапускается.
3. Проверьте: в чате попросите «сгенерируй иконку меча в `res://assets/ui/icons/sword.png`».
   Агент вызовет `tool_search comfy` → `generate_ui_icon`, панель покажет «VRAM: рендер в
   ComfyUI (шаг N/M)» и живой предпросмотр (TAESD по WebSocket).

## Инструменты агента

| Инструмент | Что делает |
| --- | --- |
| `comfy_status` / `comfy_queue` | ComfyUI онлайн?, версия, VRAM, очередь, шаблоны, чекпоинт |
| `comfy_templates` / `comfy_template_validate` | список шаблонов с плейсхолдерами; проверка нод через `/object_info` |
| `comfy_nodes_search` / `comfy_node_info` | поиск нод и их входов (для составления своих шаблонов) |
| `generate_ui_icon {prompt, save_to, size}` | иконка `size×size` → `.png` |
| `generate_pbr_material {prompt, save_to, resolution}` | `<save_to>_albedo.png` (+ `_normal`/`_roughness`, если шаблон умеет) и `<save_to>.tres` с путями `res://` |
| `generate_sprite_sheet {prompt, save_to, frames, columns, frame_size}` | кадры рендерятся батчем (не меньше 512, SD 1.5 мельче даёт шум), уменьшаются до `frame_size` и собираются в атлас; ответ содержит `hframes`/`vframes` |
| `generate_3d_prop {prompt, save_to}` | `.glb` по шаблону `prop3d_api.json` (нужны ноды Hunyuan3D-2 / TRELLIS) |
| `comfy_generate {template, params, save_to}` | любой шаблон с произвольными плейсхолдерами |
| `comfy_upload_image`, `comfy_free`, `comfy_interrupt` | референс для img2img, ручная выгрузка VRAM, прерывание |

Инструменты генерации не read-only, поэтому CodeWhale спрашивает подтверждение на каждый
вызов (в настройках можно добавить правило автоподтверждения по имени инструмента).

`save_to` принимает `res://…`: сервер ищет `project.godot` в своей рабочей папке (CodeWhale
запускает MCP-серверы в папке проекта) и выше, либо в `GODOT_PROJECT`. Промпт (и `negative`)
должен быть на английском: CLIP в SD 1.5/SDXL не понимает кириллицу, поэтому кириллица
отклоняется до рендера с подсказкой «переведи и повтори». Папка назначения создаётся до
рендера: путь, который файловая система не примет, падает сразу, а не после генерации.

## Свои шаблоны

1. Соберите и отладьте граф в ComfyUI, включите *Settings → Enable Dev mode Options*, нажмите
   **Save (API Format)**.
2. Замените значения на плейсхолдеры: текст — `{{POSITIVE_PROMPT}}` внутри строки, числа —
   строкой `"{{SEED}}"`, `"{{WIDTH}}"` (кавычки снимаются при подстановке).
3. Сохраните как `<имя>_api.json` в `<диск>\AI_Workflows`; необязательный `<имя>.meta.json`
   с `description` и `params.<ИМЯ>.default`.
4. Для PBR: ноды `SaveImage` с `filename_prefix`, содержащими `albedo`, `normal`, `roughness` —
   по ним инструмент раскладывает карты и собирает `.tres`.

## Что проверено

`codewhale-desktop/e2e/fixtures/live/final_test_comfy.py` гоняет сервер против `mock_comfy.py`
(стенд с реальными формами ответов ComfyUI v0.11): статус, шаблоны, валидация с отсутствующей
нодой `RMBG`, иконка, PBR + `.tres` с `res://`, спрайт-лист 6 кадров → атлас 4×2 128×64,
загрузка референса, free/interrupt, порядок эстафеты VRAM (`/api/generate` → `/prompt` →
`/free`) и stdio-протокол. Настоящий ComfyUI в контейнере недоступен (нет GPU и колёс
PyTorch), поэтому проверка на живом сервере — на вашей машине по шагу 3 выше.

## Проверено на машине пользователя (2026-10-19)

RTX 5060 Laptop 8 ГБ, 32 ГБ ОЗУ; ComfyUI 0.41.0 portable (`D:\AI\ComfyUI`, Python 3.13.14,
torch 2.14.0+cu130, dynamic VRAM включён), чекпоинт `v1-5-pruned-emaonly.safetensors`
(SD 1.5, 4,27 ГБ), Ollama 0.40.4 с `qwen3-4b-32k`. Шаги панели выполнены без GUI: шаблоны,
папки моделей, `extra_model_paths.yaml` и скилл — как в `comfy_install_templates`;
`settings.json → comfy.dir = D:\AI\ComfyUI\ComfyUI`, `comfy.checkpoint`; интерпретатор
`..\python_embeded\python.exe` находится сам; запись `comfy` в `mcp.json` — как в
`comfy_register_mcp`. Все инструменты вызывались через `codewhale-desktop.exe mcp-comfy call …`
(и `serve`: initialize → 14 инструментов → `comfy_status`).

| Шаг | Результат |
| --- | --- |
| Запуск ComfyUI с флагами приложения | ответ `/system_stats` через 63 с (первый запуск, импорт torch) |
| `comfy_status`, `comfy_templates`, `comfy_template_validate` ×3 | ok; все ноды и входы шаблонов совпадают с `/object_info` 0.41 — шаблоны не менялись |
| `generate_ui_icon` 512, чекпоинт на `F:` (USB T7) | 117 с, из них ~92 с загрузка модели (чтение `F:` 37 МБ/с, диск занят другими задачами), 20 шагов ≈ 3 с (~7 it/s) |
| `generate_pbr_material` 512 / `generate_sprite_sheet` 4×128, с `F:` | 117 с / 135 с |
| то же с копией чекпоинта на NVMe (`D:\AI\ComfyUI\ComfyUI\models\checkpoints`) | иконка 12,4 с, PBR 12,8 с, спрайт-лист 4×512 18 с |
| Эстафета VRAM | `qwen3-4b-32k` (5,0 ГБ VRAM) выгружается за ~1 с: 7,05 → 2,2 ГБ занято; пик рендера ~4,6 ГБ; после `/free` ~1,95 ГБ; `unloadedLlm` и `freedVram: true` в ответе; повторная загрузка модели Ollama ~8,6 с |

Замечания:
- Модель ComfyUI после `/free` (`unload_models`) не остаётся в ОЗУ, если свободной памяти мало
  (WSL/Docker держали ~13 ГБ): каждая генерация заново читает чекпоинт с диска. Поэтому
  рабочий чекпоинт держите на внутреннем NVMe — ComfyUI сначала ищет в `models\checkpoints`,
  потом в `AI_ComfyModels` с диска знаний. Копия на `D:` оставлена; удалите её, чтобы снова
  брать файл с `F:`.
- SD 1.5 не умеет кадры 128×128: с `frame_size` 128 получался шум. Исправлено (2026-10-20):
  кадры рендерятся минимум в 512 и уменьшаются до `frame_size` при сборке атласа. Кадры
  батча — независимые картинки, а не фазы одной анимации.
- В Git Bash обратные слэши в JSON для `call` нужно удваивать ещё раз (`"D:\\\\AI\\\\x.png"`),
  иначе JSON не разберётся и инструмент скажет «Нужен параметр prompt».
- Запущенное приложение держит настройки в памяти: после ручной правки `settings.json`
  перезапустите его, иначе следующее сохранение настроек вернёт `comfy` к значениям по умолчанию.

## Живой агент: локальная модель вызывает ComfyUI (2026-10-20)

Драйвер на основе `final_test_water.py`: временный `CODEWHALE_HOME` с конфигом пользователя,
`[tools] always_load` = `mcp_comfy_generate_ui_icon`, `…_generate_pbr_material`,
`…_generate_sprite_sheet`, `…_comfy_status`; в `mcp.json` только `comfy`; скилл
`comfyui-gamedev`; рабочая папка — копия `Godot/WaterDemo`; все подтверждения «allow» (кроме
Web). Модель `qwen3-4b-32k`, по одному ходу на запрос. Результаты и картинки:
`D:\AI\cw-bench\comfy-agent\`.

| Запрос | Сервер из portable-сборки 0.1.0 | После исправлений (`res://`, английский промпт, кадры ≥ 512) |
| --- | --- | --- |
| Иконка меча `res://assets/ui/icons/sword.png`, 512 | `generate_ui_icon` вызван сразу, но промпт по-русски и `save_to` = `res://…`: рендер прошёл (15 с), запись упала `os error 123`, файла нет; модель объяснила ошибку и спросила разрешения | файл есть; ход 37 с, рендер 10 с; промпт по-английски, но «isometric view, dark stone background» — иконка посредственная |
| PBR мокрой мостовой + подключить к плоскости | `generate_pbr_material` с `resolution` 2048 и русским промптом: 124 с рендера (пик 7,85 из 8 ГБ VRAM) впустую, затем `os error 123`; модель записала мусорный файл `res/assets/materials/cobblestone` и дважды неудачно правила `.tscn` | `cobblestone_albedo.png` 1024 + `.tres` есть, ход 38 с (рендер 20 с); к плоскости **не подключила** — ни 4B, ни `qwen3-8b-16k` (8B дал инструкцию «выберите плоскость в редакторе»); плоскости строятся в `water_demo.gd`, в `.tscn` их нет |
| Спрайт-лист 4 кадра огненного элементаля | `generate_sprite_sheet` (русский промпт, `res://` → ошибка), повтор с `res/assets/…` удался, но кадры 128 — шум, файл в `res://res/assets/…`, плюс лишний `.txt`; модель отчиталась об успехе | файл `assets/sprites/fire_elemental.png` 512×128 (4×128), ход 34 с, рендер 16 с; четыре узнаваемых огненных элементаля, но не фазы одной анимации |

Эстафета VRAM во всех 8 генерациях: модель Ollama пропадает из `/api/ps` через 0,4–0,8 с после
начала вызова (занято 5,6–7,6 ГБ → 0,4–1,4 ГБ), пик рендера 512 ≈ 3,3–4,1 ГБ, 1024 ≈ 4,7 ГБ,
2048 ≈ 7,85 ГБ; после `/free` модель Ollama загружается снова при следующем запросе
(видна в `/api/ps` через 4,4–8,5 с после конца вызова; этот запрос идёт 11–12 с вместо 1–2 с).

## Ограничения
- `--lowvram` в новых ComfyUI не действует при включённом dynamic VRAM — это нормально,
  `--reserve-vram 1.0` работает всегда.
- Прозрачный фон у иконок и карты normal/roughness требуют кастомных нод (RMBG-2.0,
  Marigold/Lotus) — добавьте их в шаблоны; базовые шаблоны работают на чистом ComfyUI.
- Имя сервера в списке модели — `mcp_comfy_*`; в подтверждениях — то же.
