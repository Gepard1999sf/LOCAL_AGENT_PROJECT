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

## Диск F: как диск знаний (пошагово)

Корень диска знаний — любая папка на большом диске; `D:\AI_Drive` в примерах выше — лишь значение
по умолчанию. Если свободное место у вас на `F:` (там же, где `F:\PROJECTS\godot-hd`), всё
делается так же, только с `F:\AI_Drive`. Папку лучше держать отдельно от проектов и от Godot:
скрипт наполнения кладёт в неё десятки гигабайт (репозитории, дампы, модели).

1. **Место.** На первый проход нужно ~20–30 ГБ (модели «золотого набора» + 4 репозитория Godot),
   с `-Math` ещё ~15–25 ГБ дампов StackExchange и учебников, с `-Full` (весь StackOverflow)
   ещё ~90 ГБ. Проверить: `Get-PSDrive F`.
2. **Настройки приложения.** `Ctrl+,` → **Диск знаний и Godot** → папка диска `F:\AI_Drive`,
   путь к Godot `F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe`. Файл
   настроек: `%APPDATA%\codewhale-desktop\settings.json` (ключи `knowledgeDrive.root`, `godotPath`).
3. **Подготовить диск** (`Ctrl+K` → «Диск знаний» → кнопка). Появятся папки `models`, `docsets`,
   `git_vault`, `kiwix_zim`, `math_physics`, `vectordb`, `bin`, `index`, `.agent_state` и файлы
   `F:\AI_Drive\bin\sympy_helper.py`, `F:\AI_Drive\bin\init-ai-drive.ps1`.
4. **Наполнение** (PowerShell 5.1, можно частями; скрипт идемпотентный, прерванные загрузки
   докачиваются через BITS):
   ```powershell
   # первый проход: модели + репозитории, без дампов Kiwix
   powershell -NoProfile -ExecutionPolicy Bypass -File F:\AI_Drive\bin\init-ai-drive.ps1 -DriveRoot F:\AI_Drive -SkipZim
   # потом, когда будет время: дампы GameDev SE и Blender SE
   powershell -NoProfile -ExecutionPolicy Bypass -File F:\AI_Drive\bin\init-ai-drive.ps1 -DriveRoot F:\AI_Drive -SkipModels -SkipRepos
   # математика и физика (Math SE, Physics SE, OpenStax, Jolt/Box2D/GLM)
   powershell -NoProfile -ExecutionPolicy Bypass -File F:\AI_Drive\bin\init-ai-drive.ps1 -DriveRoot F:\AI_Drive -SkipModels -SkipRepos -Math
   ```
   Что делает скрипт: создаёт структуру, ставит пользовательскую переменную
   `OLLAMA_MODELS=F:\AI_Drive\models`, скачивает модели через `ollama pull` (теги «золотого
   набора» сверяйте с ollama.com/library и с `docs/research/rtx5060-8gb-model-plan.md` — для
   8 ГБ VRAM не все из них имеют смысл, лишние можно пропустить ключом `-SkipModels` и скачать
   нужные из панели «Модели Ollama»), клонирует `godot`, `godot-demo-projects`, `godot-docs`,
   пример third-person-контроллера (`--depth 1`), скачивает `.zim` с download.kiwix.org и
   `kiwix-tools` для Windows в `bin`, ставит SymPy.
5. **Докcеты Zeal.** Установить Zeal (zealdocs.org), в нём скачать Godot, C++, Python, GLSL/OpenGL,
   затем скопировать папки `*.docset` из `%APPDATA%\Zeal\Zeal\docsets` в `F:\AI_Drive\docsets`.
   Это даёт инструменты `docset_search` / `docset_read`.
6. **Модели на диске (по желанию).** Если модели Ollama уже лежат в `%USERPROFILE%\.ollama\models`,
   их можно перенести: кнопка **OLLAMA_MODELS → диск** (или `robocopy` как в `PROJECT.md`,
   раздел 8.1), затем **Перезапустить Ollama**. Внешний диск медленнее NVMe: первая загрузка
   модели после выгрузки станет дольше, поэтому на SSD это выгодно, на HDD — только ради места.
7. **Построить индекс** — один раз после клонирования `git_vault` (FTS5-индекс
   `F:\AI_Drive\index\git_vault.sqlite`, несколько минут). Повторять после обновления репозиториев.
8. **Запустить Kiwix** — когда в `kiwix_zim` есть `.zim`, а в `bin` — `kiwix-serve.exe`; сервер
   поднимается на `127.0.0.1:8090` (порт в настройках).
9. **Установить скиллы** (`knowledge-drive`, `math-physics-godot` → `~/.codewhale/skills`) и
   **Подключить MCP** (запись `ai-drive` в `%USERPROFILE%\.codewhale\mcp.json` с
   `AI_DRIVE_PATH=F:\AI_Drive`; CodeWhale перезапустится).
10. **Проверка** из консоли (путь к exe показывает панель, строка «MCP-сервер»):
    ```powershell
    & "<путь>\codewhale-desktop.exe" mcp-drive status
    & "<путь>\codewhale-desktop.exe" mcp-drive call docset_search '{"query":"Vector3.slerp"}'
    & "<путь>\codewhale-desktop.exe" mcp-drive call code_search '{"query":"gerstner"}'
    & "<путь>\codewhale-desktop.exe" mcp-drive call godot_check '{"path":"F:\\PROJECTS\\<проект>\\water.gdshader"}'
    ```
    В чате: «найди в документации CharacterBody3D.move_and_slide» → карточки `tool_search` →
    `mcp_ai--drive_docset_search`.

Что получает агент после этого: точную документацию Godot/C++/Python без интернета, поиск по
исходникам движка и демо-проектам, ответы GameDev/Math/Physics StackExchange, символьную математику
(`math_eval`), проверку `.gd`/`.gdshader` без окна (`godot_check`) и журнал выполненных задач
(`ledger.json` в `F:\AI_Drive\.agent_state`).

## Как это работает

- `mcp.json` получает запись вида
  ```json
  { "servers": { "ai-drive": { "command": "C:\\…\\codewhale-desktop.exe", "args": ["mcp-drive"],
      "env": { "AI_DRIVE_PATH": "D:\\AI_Drive", "KIWIX_URL": "http://127.0.0.1:8090", "GODOT_BIN": "…" } } } }
  ```
  CodeWhale запускает сервер сам (stdio, JSON-RPC). Инструменты MCP «отложенные»: модель сначала
  вызывает `tool_search` с именем сервера (`ai-drive`), после чего они появляются в её списке как
  `mcp_ai--drive_*` (дефис в имени сервера удваивается); в запросах на подтверждение они называются
  `mcp_ai-drive_*`.
- CodeWhale спрашивает подтверждение на каждый MCP-инструмент пользовательского сервера, даже
  помеченный `readOnlyHint`. Чтобы агент не дёргал вас на каждый поиск, в приложении включено
  правило автоподтверждения **ai-drive** (Настройки → Агент); отключите его, если хотите
  подтверждать каждый вызов вручную.
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
