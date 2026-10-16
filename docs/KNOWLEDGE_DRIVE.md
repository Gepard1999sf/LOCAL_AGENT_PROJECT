# Диск знаний: оффлайн-база для агента

Съёмный диск (1 ТБ) превращает локальную модель 7–30B в агента с точной документацией,
исходниками движков, миллионами ответов StackOverflow и символьной математикой под рукой.
CodeWhale Desktop даёт агенту доступ к диску через встроенный MCP-сервер `ai-drive`
(это то же самое приложение, запущенное как `codewhale-desktop.exe mcp-drive`).

## Что лежит на диске

| Папка | Содержимое | Инструменты агента |
| --- | --- | --- |
| `models` | модели Ollama (`OLLAMA_MODELS`) | — |
| `docsets` | папки `*.docset` из Zeal/Dash (Godot 4, C++, Python, GLSL…) | `docsets_list`, `docset_search`, `docset_read` |
| `git_vault` | эталонные репозитории (`godot`, `godot-demo-projects`, Jolt, Box2D…) | `code_search` (по индексу FTS5), `code_read` |
| `kiwix_zim` | дампы `.zim`: StackOverflow, GameDev SE, Math SE, Physics SE | `kiwix_books`, `kiwix_search`, `kiwix_read` (через `kiwix-serve`) |
| `math_physics` | учебники, архивы статей (OpenStax, Inigo Quilez, Red Blob) | через `kiwix_*` и `code_*` |
| `vectordb` | таблицы LanceDB (позже) | — |
| `bin` | `kiwix-serve.exe`, `sympy_helper.py`, `init-ai-drive.ps1` | `math_eval` |
| `index` | `git_vault.sqlite` — полнотекстовый индекс, строится приложением | `code_search` |
| `.agent_state` | `bookmarks.json`, `ledger.json` | `log_read_since_bookmark`, `ledger_record` |

Плюс инструмент `godot_check` (проверка `.gd` и `.gdshader` без окна) и `ledger_record`,
который записывает задачу как выполненную только после успешной проверки Godot.

## Первая настройка (один раз)

1. Подключите диск, в приложении откройте **Настройки → Диск знаний и Godot**: укажите папку
   диска (по умолчанию `D:\AI_Drive`) и путь к `godot.exe`.
2. Откройте панель **Диск знаний** (палитра `Ctrl+K` → «Диск знаний») и нажмите
   **Подготовить диск**: создаются папки, в `bin` копируются `sympy_helper.py` и `init-ai-drive.ps1`.
3. Наполните диск скриптом (PowerShell 5.1):
   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File D:\AI_Drive\bin\init-ai-drive.ps1 -DriveRoot D:\AI_Drive
   ```
   Ключи: `-Math` (база математики и физики: Math SE, Physics SE, учебники, Jolt/Box2D/GLM),
   `-Full` (полный StackOverflow, ~90 ГБ), `-SkipModels`, `-SkipRepos`, `-SkipZim`.
   Скрипт идемпотентный: повторный запуск докачивает только недостающее (BITS умеет докачку).
4. Докcеты Zeal: установите Zeal (zealdocs.org), скачайте нужные докcеты и скопируйте папки
   `*.docset` из `%APPDATA%\Zeal\Zeal\docsets` в `D:\AI_Drive\docsets`.
5. В панели **Диск знаний**:
   - **OLLAMA_MODELS → диск**, затем **Перезапустить Ollama** — модели будут храниться на диске;
   - **Построить индекс** — один раз после наполнения `git_vault` (и после обновления репозиториев);
   - **Запустить Kiwix** — поднимает `kiwix-serve` на `127.0.0.1:8090` для всех `.zim`;
   - **Установить скиллы** — `knowledge-drive` и `math-physics-godot` попадают в `~/.codewhale/skills`
     (или **…в проект** → `.agents/skills` текущего проекта);
   - **Подключить MCP** — запись `ai-drive` добавляется в `~/.codewhale/mcp.json`, CodeWhale
     перезапускается и видит инструменты.
6. Установите SymPy: `pip install sympy` (скрипт из п. 3 делает это сам).

## Как это работает

- `mcp.json` получает запись вида
  ```json
  { "servers": { "ai-drive": { "command": "C:\\…\\codewhale-desktop.exe", "args": ["mcp-drive"],
      "env": { "AI_DRIVE_PATH": "D:\\AI_Drive", "KIWIX_URL": "http://127.0.0.1:8090", "GODOT_BIN": "…" } } } }
  ```
  CodeWhale запускает сервер сам (stdio, JSON-RPC) и показывает инструменты модели как `mcp_ai-drive_*`.
- Инструменты поиска помечены `readOnlyHint`, поэтому в режиме Ask они не требуют подтверждения;
  `ledger_record`, `log_read_since_bookmark` и `log_reset_bookmark` меняют файлы в `.agent_state`
  и подтверждаются как обычные инструменты.
- Ответы инструментов обрезаются на 12 000 символах — скилл учит агента сужать запросы и читать
  диапазоны строк, а не файлы целиком.
- Закладки логов: `log_read_since_bookmark` возвращает только новые строки с момента прошлого
  вызова (ключ `source_key`), а при перезаписи файла движком начинает с начала.
- Проверка Godot: `.gdshader` компилируется в headless-проекте приложения (`resources/godot-check`),
  `.gd` — через `godot --headless --check-only --script`. Ошибки возвращаются списком.

## Проверка вручную

```powershell
# состояние диска и доступность Kiwix/Godot/SymPy
& "C:\Users\<вы>\AppData\Local\CodeWhale Desktop\codewhale-desktop.exe" mcp-drive status
# вызвать инструмент напрямую
& "...\codewhale-desktop.exe" mcp-drive call docset_search '{"query":"Vector3.slerp"}'
& "...\codewhale-desktop.exe" mcp-drive call godot_check '{"path":"D:\\Proj\\water.gdshader"}'
# перестроить индекс из консоли
& "...\codewhale-desktop.exe" mcp-drive index --root D:\AI_Drive
```

## Ограничения

- Теги моделей в «золотом наборе» (`qwen3:30b-a3b`, `qwen2.5-coder:14b`, `deepseek-r1:14b`,
  `bge-m3`) взяты из плана; перед скачиванием сверьтесь с ollama.com/library — теги меняются.
- `kiwix-serve` и `kiwix-tools` для Windows скачиваются с download.kiwix.org; версия в
  имени архива меняется, скрипт берёт последнюю из листинга.
- Индекс `git_vault` хранит текст файлов до 512 КБ; бинарные файлы и папки `.git`, `node_modules`,
  `target`, `.godot`, `build` пропускаются. Поиск делит идентификаторы по `_`: запрос `gerstner`
  найдёт `apply_gerstner_wave`, `set_global*` — `set_global_position`.
