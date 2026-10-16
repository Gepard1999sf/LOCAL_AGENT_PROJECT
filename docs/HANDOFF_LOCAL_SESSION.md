# Передача в локальную сессию Claude Code (Windows, машина пользователя)

Этот документ — задание для сессии Claude Code, запущенной **на компьютере пользователя**
в папке репозитория (ветка `claude/optimistic-ride-hu7e74`, например
`C:\Users\Stryker\Downloads\LOCAL_AGENT_PROJECT-claude-optimistic-ride-hu7e74`
или свежий `git clone`). Облачная сессия (Linux, без GPU, без Windows SDK, без реестра Ollama)
довела код до зелёных проверок, но всё, что требует Windows, видеокарты, живой модели, Godot и
ComfyUI, проверяется только здесь. Выполняй разделы по порядку, каждый заканчивается
критерием «готово».

Правила проекта — в `AGENTS.md` (отвечать по-русски, код и имена по-английски, оболочка —
Windows PowerShell 5.1: без `&&`, без grep/sed; Godot 4.8 лежит в
`F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe`).

## 0. Что уже сделано (не переделывать)

| Компонент | Где | Проверено в облаке |
| --- | --- | --- |
| Десктоп-клиент CodeWhale (Tauri 2 + React) | `codewhale-desktop/` | tsc, vite build, 33 e2e (моки IPC), clippy, 47 Rust-тестов; CI на `windows-latest` зелёный |
| MCP-сервер `ai-drive` (диск знаний) | `src-tauri/src/drive_mcp.rs`, команды `src-tauri/src/drive.rs`, панель `src/components/drive/` | сквозной тест с настоящим CodeWhale + Godot 4.7.2 headless (`docs/FINAL_TEST_WATER.md`, `docs/final-test/water.png`) |
| MCP-сервер `comfy` (фабрика ассетов) | `src-tauri/src/comfy_mcp.rs`, `src-tauri/src/comfy.rs`, панель `src/components/comfy/` | против стенда `mock_comfy.py` с формами API ComfyUI v0.11 (`final_test_comfy.py`, 17/17) |
| Скиллы, шаблоны, скрипты | `src-tauri/resources/` (skills/, comfy/, drive/, godot-check/) | синтаксис, юнит-тесты |
| Демо-проект воды | `Godot/WaterDemo/` | шейдер компилируется, рендер под Xvfb |
| Установщик/portable | `.github/workflows/release.yml` (NSIS setup, MSI, portable zip) | запуск №1 через workflow_dispatch — см. раздел 7 |
| Документация | `docs/INSTALL.md`, `docs/KNOWLEDGE_DRIVE.md`, `docs/COMFYUI.md`, `docs/FINAL_TEST_WATER.md`, `PROJECT.md` | — |

Исследовательские материалы агентов лежат в `docs/research/` (JSON/MD): их синтез (модель
безопасности, план моделей под RTX 5060 8 ГБ, верификация PDF) не завершён — это отдельная
задача, не блокирует запуск.

## 1. Проверка окружения (10 минут)

Выполни и запиши результат каждой команды:
```powershell
node -v            # нужен 22.x
npm -v
rustc -vV          # host: x86_64-pc-windows-msvc; если нет rustup — https://rustup.rs, toolchain stable-msvc
cargo --version
ollama --version   # Ollama установлена и запущена (трей); иначе https://ollama.com/download
ollama list        # хотя бы одна модель с вызовом инструментов, напр. qwen2.5-coder:7b
python --version   # Python 3.11+; затем: python -c "import sympy; print(sympy.__version__)"
git --version
Test-Path "F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe"
Get-AppxPackage -Name "*WebView2*" | Select-Object Name, Version   # пусто — установить Evergreen WebView2
nvidia-smi --query-gpu=name,memory.total --format=csv
```
Нет Visual Studio Build Tools (C++ workload, Windows SDK) — поставить: без них не соберётся
`rusqlite` (SQLite компилируется из исходников) и Tauri. Нет `sympy` — `pip install sympy`.

Критерий: все команды выполнены, недостающее установлено.

## 2. Сборка и первый запуск приложения

```powershell
# один раз: папка сборки Rust на диске с местом (3–6 ГБ)
[Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', 'D:\AI\build\codewhale-desktop-target', 'User')
# открыть НОВОЕ окно PowerShell после этого
cd codewhale-desktop
npm ci
npm i -g codewhale          # агент; либо укажи путь к codewhale.exe в настройках приложения
npm run tauri dev           # первая сборка Rust 10–20 минут
```
Ожидается окно «CodeWhale Desktop»: статус «Запуск CodeWhale» → чат. Если runtime не
стартует — журнал: палитра `Ctrl+K` → «Журнал CodeWhale»; ошибки сборки — чинить в коде и
коммитить.

Критерий: приложение открывается, в заголовке видна модель Ollama, простой запрос в чате
получает ответ.

## 3. Настройки приложения

Настройки (`Ctrl+,`):
- **Ollama → Модель по умолчанию**: модель с инструментами и контекстом ≥ 16k (панель
  «Модели Ollama» умеет создать вариант с нужным `num_ctx`; контекст 4k по умолчанию — ловушка).
- **Диск знаний и Godot**: папка диска (`D:\AI_Drive` или другая), путь к `godot.exe`.
- **Агент → автоподтверждение**: правило **ai-drive** включено по умолчанию (иначе каждый
  поиск по диску просит подтверждение).

Критерий: настройки сохранены (файл `%APPDATA%\codewhale-desktop\settings.json`).

## 4. Диск знаний

Панель `Ctrl+K` → «Диск знаний»:
1. **Подготовить диск** → в `<диск>\bin` появятся `sympy_helper.py` и `init-ai-drive.ps1`.
2. Наполнение (долго, можно частями):
   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File D:\AI_Drive\bin\init-ai-drive.ps1 -DriveRoot D:\AI_Drive -SkipZim
   ```
   `-SkipZim` на первом проходе; дампы Kiwix (-Math, -Full) — отдельно, когда будет время.
   Докcеты Zeal: установить Zeal, скачать Godot/C++/Python/GLSL, скопировать папки `*.docset`
   из `%APPDATA%\Zeal\Zeal\docsets` в `D:\AI_Drive\docsets`.
3. **OLLAMA_MODELS → диск** → **Перезапустить Ollama** (если модели нужны на внешнем диске;
   иначе пропустить).
4. **Построить индекс** (после клонирования репозиториев в `git_vault`).
5. **Запустить Kiwix** (если есть `.zim` и `kiwix-serve.exe` в `bin`).
6. **Установить скиллы** → `~/.codewhale/skills/knowledge-drive`, `math-physics-godot`.
7. **Подключить MCP** → запись `ai-drive` в `%USERPROFILE%\.codewhale\mcp.json`, CodeWhale
   перезапустится.

Проверка из консоли (путь к exe — из панели, строка «MCP-сервер»):
```powershell
& "D:\AI\build\codewhale-desktop-target\debug\codewhale-desktop.exe" mcp-drive status
& "...\codewhale-desktop.exe" mcp-drive call docset_search '{"query":"Vector3.slerp"}'
& "...\codewhale-desktop.exe" mcp-drive call godot_check '{"path":"<абсолютный путь>\\Godot\\WaterDemo\\water.gdshader"}'
```
Критерий: `status` показывает разделы, `godot_check` возвращает `"ok": true, "uniforms": 11`;
в чате запрос «найди в документации CharacterBody3D.move_and_slide» приводит к вызовам
`tool_search` → `mcp_ai--drive_docset_search` (видно в карточках инструментов).

## 5. Финальный тест: шейдер воды с настоящей моделью

По `docs/FINAL_TEST_WATER.md`, раздел «Как повторить у себя»: открыть проект
`Godot/WaterDemo` в приложении, отправить промпт из документа. Ожидаемый ход: агент
подгружает скилл, пишет `water.gdshader`, вызывает `godot_check`, правит до `ok: true`,
вызывает `ledger_record`. Затем рендер:
```powershell
& "F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe" --path .\Godot\WaterDemo --resolution 1280x720 --quit-after 120 -- --shot D:\water.png
```
Критерий: `D:\water.png` похож на `docs/final-test/water.png`; запись в
`D:\AI_Drive\.agent_state\ledger.json`. Если модель не справляется (не вызывает инструменты,
путает имена) — зафиксировать, какая модель и что именно пошло не так, попробовать
`qwen2.5-coder:7b`/`14b` с `num_ctx` 32k.

## 6. ComfyUI: фабрика ассетов

По `docs/COMFYUI.md`: папка ComfyUI с `main.py`, чекпоинт в `D:\AI_Drive\AI_ComfyModels\checkpoints`
или `ComfyUI\models\checkpoints`, панель «Фабрика ассетов» → **Установить шаблоны** →
выбрать чекпоинт → **Запустить ComfyUI** → **Подключить MCP**.

Проверка из консоли (без агента):
```powershell
$env:COMFY_URL='http://127.0.0.1:8188'; $env:AI_WORKFLOWS_DIR='D:\AI_Drive\AI_Workflows'; $env:COMFY_CHECKPOINT='<файл чекпоинта>'
& "...\codewhale-desktop.exe" mcp-comfy call comfy_template_validate '{"template":"ui_icon"}'
& "...\codewhale-desktop.exe" mcp-comfy call generate_ui_icon '{"prompt":"iron sword","save_to":"D:\\sword.png","size":512}'
```
Во время рендера в панели должен смениться индикатор «VRAM: рендер в ComfyUI (шаг N/M)» и
появиться предпросмотр; `nvidia-smi` покажет, что модель Ollama выгружена. Затем в чате:
«сгенерируй иконку меча в `res://assets/ui/icons/sword.png`».

Критерий: файл создан, `freedVram: true`, после генерации модель Ollama снова отвечает без
долгой паузы. Если ComfyUI падает по памяти — уменьшить `size`, проверить `--reserve-vram`,
записать модель/чекпоинт/лог.

## 7. Установщик и portable

Облачная сессия запустила `Release` (workflow_dispatch, run #1) на GitHub Actions; результат
смотри в Actions → Release: артефакты `CodeWhale-Desktop_<ver>_x64-setup.exe`, `.msi`,
`*-portable.zip`. Если запуск красный — логи в Actions, починить `release.yml`/код, закоммитить,
перезапустить (Actions → Release → Run workflow на ветке). Локальная альтернатива:
```powershell
cd codewhale-desktop
npm run bundle    # NSIS + MSI в <CARGO_TARGET_DIR>\release\bundle\
```
Когда сборка зелёная, выпустить релиз тегом (установщики появятся в GitHub Releases без
логина):
```powershell
git tag v0.1.0
git push origin v0.1.0
```
Критерий: установщик ставится без прав администратора, приложение запускается; portable-zip
распаковывается и запускается; SmartScreen обходится через «Подробнее → Выполнить в любом случае».

## 8. Прочие хвосты (после основного)

- `src/components/models/ModelManager.tsx` (полный менеджер моделей: загрузка, удаление,
  варианты, оценка VRAM) написан и команды `models_*` зарегистрированы, но в оверлее
  «models» всё ещё старая `ModelsPanel`. Подключить `ModelManager`, добавить моки `models_*`
  в `e2e/mock/backend.ts`, прогнать e2e.
- e2e на Windows: `cd codewhale-desktop\e2e; npm ci; npx playwright install chromium; npx playwright test`.
- `release.yml` не проходил независимое ревью; `tools\check-shader.ps1` ссылается на проект
  `D:\AI\godot-shader-check` — его можно заменить на `codewhale-desktop\src-tauri\resources\godot-check`.
- Исследования в `docs/research/` ждут синтеза (безопасность: `codewhale-safety-design`;
  модели под RTX 5060: `rtx5060-8gb-model-plan`; верификация PDF пользователя).
- Контейнерные находки, которые стоит учитывать: MCP-инструменты в CodeWhale отложенные
  (`tool_search` обязателен), имена видны модели как `mcp_ai--drive_*`/`mcp_comfy_*`;
  `readOnlyHint` не снимает подтверждение для пользовательских серверов; в Compatibility-рендере
  Godot глубина из `hint_depth_texture` требует `depth * 2.0 - 1.0`.

## 9. Как отчитываться

В конце — короткий отчёт в `docs/LOCAL_RUN_REPORT.md`: таблица разделов 1–7 с «готово /
не готово / почему», версии (Windows, Node, Rust, Ollama, модель, ComfyUI, Godot), что
поменяно в коде. Коммиты — в ту же ветку `claude/optimistic-ride-hu7e74`, небольшими
осмысленными порциями, с проверкой `npx tsc --noEmit` и `cargo clippy -- -D warnings`
(в `src-tauri`) перед каждым.
