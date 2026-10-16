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
| `generate_sprite_sheet {prompt, save_to, frames, columns, frame_size}` | кадры рендерятся батчем и собираются в атлас; ответ содержит `hframes`/`vframes` |
| `generate_3d_prop {prompt, save_to}` | `.glb` по шаблону `prop3d_api.json` (нужны ноды Hunyuan3D-2 / TRELLIS) |
| `comfy_generate {template, params, save_to}` | любой шаблон с произвольными плейсхолдерами |
| `comfy_upload_image`, `comfy_free`, `comfy_interrupt` | референс для img2img, ручная выгрузка VRAM, прерывание |

Инструменты генерации не read-only, поэтому CodeWhale спрашивает подтверждение на каждый
вызов (в настройках можно добавить правило автоподтверждения по имени инструмента).

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

## Ограничения
- `--lowvram` в новых ComfyUI не действует при включённом dynamic VRAM — это нормально,
  `--reserve-vram 1.0` работает всегда.
- Прозрачный фон у иконок и карты normal/roughness требуют кастомных нод (RMBG-2.0,
  Marigold/Lotus) — добавьте их в шаблоны; базовые шаблоны работают на чистом ComfyUI.
- Имя сервера в списке модели — `mcp_comfy_*`; в подтверждениях — то же.
