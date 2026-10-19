# Отчёт о локальном прогоне (Windows, машина пользователя)

Заполняется локальной сессией по `docs/HANDOFF_LOCAL_SESSION.md`, раздел 9. Строки со статусом
«не проверено» — то, что требует Windows, GPU, живой модели, Godot и ComfyUI и в облачном
контейнере (Linux) выполнить нельзя. Колонка «Проверено в облаке 2026-10-16» — что уже
подтверждено без машины пользователя.

## Разделы 1–7

| Раздел | Статус | Почему / что проверить | Проверено в облаке 2026-10-16 |
| --- | --- | --- | --- |
| 0a. Порядок на ПК (аудит, сортировка) | готово (кроме трёх ручных удалений и переноса репозитория) | аудит `Invoke-PcAudit.ps1` прошёл за ~3 мин (отчёт `Desktop\PC-Audit\PC-Audit-2026-10-16_20-35.md`): C: 476 ГБ, свободно было 24 ГБ; Epic Games 98 ГБ, Docker 54 ГБ, кэш шейдеров NVIDIA 33 ГБ, Unity 36 ГБ, дампы WSL 19 ГБ. `Invoke-PcOrganize.ps1 -Root F:\ -All -Apply` с согласия пользователя: 341 перемещение, 0 проблем (журнал `PC-Organize-Journal-2026-10-16_20-57.csv`, откат `-Undo`). `Invoke-PcRecycle.ps1 -Apply`: 38 кандидатов в Корзине; `C:\AMD` и `C:\inetpub` требуют прав администратора, `F:\Docker` (старый образ 53 ГБ) больше лимита Корзины — удалить вручную. Свободно на C: стало 46 ГБ до очистки Корзины. Репозиторий остаётся в `Downloads` до конца сессии (`-MoveRepo`), SSH-ключи `Desktop\Octagon-Servers` не тронуты | скрипты написаны и проверены локальной сессией на Windows 11 (коммит `e9769c5`) |
| 1. Проверка окружения | готово | Node 26.4.0 / npm 11.17.0, rustc 1.99.0 msvc, cargo 1.99.0, git 2.54.0, Python 3.14.6 (`py`; `python` — алиас Store), sympy 1.14.0 (поставлен), Ollama 0.40.4 (12 моделей, в т. ч. `qwen2.5-coder:7b`, `qwen3-4b-32k`), Godot по пути `F:\PROJECTS\godot-hd\…` есть, WebView2 Evergreen 155.0.4283.45, RTX 5060 Laptop 8151 MiB (драйвер 610.62), VS Build Tools 2022 + Windows SDK 10.0.26100, CodeWhale 0.10.1. `CARGO_TARGET_DIR` (User) = `D:\AI\build\codewhale-desktop-target` — установлен | — |
| 2. Сборка и первый запуск | частично | `npm ci` (246 пакетов), `npx tsc --noEmit`, `npm run build` — зелёные; `npm run tauri dev` собрал Rust за 1 мин 47 с (инкрементально, target на D:) и запустил `codewhale-desktop.exe`, `codewhale-app-server.exe` поднялся. Окно, модель в заголовке и ответ в чате не проверены: приложение было закрыто до визуальной проверки. На Windows также зелёные `cargo clippy -- -D warnings` и `cargo test` (47 тестов) | `npx tsc --noEmit`, `npm run build` (Vite), `cargo clippy -- -D warnings`, `cargo test` (47 тестов) — зелёные под Linux |
| 3. Настройки приложения | готово | `%APPDATA%\net.codewhale.desktop\settings.json` (резервная копия `settings.json.bak-20261016`): `defaultModel` = `qwen3-4b-32k:latest` (инструменты, `num_ctx` 32768), `knowledgeDrive.root` = `F:\AI_Drive`, `godotPath` = `F:\PROJECTS\godot-hd\…\godot.windows.editor.x86_64.exe`, `pythonPath` = Python 3.14 (не алиас Store), `autoApprove.ai_drive` = true, `workspace` = `F:\AI\CodeWhale\LOCAL_AGENT_PROJECT\Godot\WaterDemo`. Записано напрямую в JSON при закрытом приложении; в окне настроек не проверялось | — |
| 4. Диск знаний | частично | Готово без GUI: структура `F:\AI_Drive` (все разделы + `.agent_state`), `bin\init-ai-drive.ps1` и `bin\sympy_helper.py`, скиллы `knowledge-drive` и `math-physics-godot` в `~/.codewhale/skills`, запись `ai-drive` в `~/.codewhale/mcp.json` (команда — portable `codewhale-desktop.exe mcp-drive`, env `AI_DRIVE_PATH=F:\AI_Drive`, `GODOT_BIN`, `PYTHON_BIN`, `SYMPY_HELPER`). Проверка из консоли: `mcp-drive status` показывает разделы, Godot и SymPy найдены; `godot_check` на `Godot\WaterDemo\water.gdshader` → `"ok": true, "uniforms": 11`; `math_eval integrate sin(x)**2` → `x/2 - sin(2*x)/4`. `OLLAMA_MODELS` оставлена `D:\AI\ollama\models` (ключ `-KeepOllamaModels`). Не сделано: наполнение (репозитории, дампы Kiwix, докcеты — ждёт решения о загрузке ~5–6 ГБ), индекс git_vault, kiwix-serve, проверка `tool_search → mcp_ai--drive_docset_search` в чате | — |
| 5. Финальный тест: шейдер воды | не пройден, причины найдены | Драйвер `e2e/fixtures/live/final_test_water.py --ollama-url` гоняет настоящий CodeWhale с `qwen3-4b-32k`. **Прогон 1** (2026-10-16): агент загрузил скилл, прочитал сцену и материал, записал шейдер на 146 строк без `shader_type` с GLSL-конструкциями; следующий вызов инструмента оборвался по `[stream] open_timeout_secs` = 45 с (подробности ниже). **Прогон 2** (2026-10-17, таймаут 300 с): обрывов нет; агент дважды записал шейдер на 948 байт (снова без `shader_type`), вызвал `tool_search`, но запросы не вернули инструменты `ai-drive` (первый — пусто, второй — `Web` и `apply_patch`), после чего 20 минут искал «godot_check» в вебе; контекст сжимался каждые ~20 с на ~7,5 тыс. токенов, потому что временный дом CodeWhale в драйвере не знал окна 32k; к концу модель потеряла смысл `res://` и создала папку `res/`. Ход не завершился за 1500 с. Отметка драйвера «godot_check вызван» была ложной (совпадение по тексту веб-запросов) — исправляется | сквозной тест с `mock_ollama.py` (`docs/FINAL_TEST_WATER.md`) |
| 6. ComfyUI: фабрика ассетов | не проверено | `comfy_template_validate`, `generate_ui_icon`, индикатор VRAM, `freedVram: true` | 17/17 против стенда `mock_comfy.py` |
| 7. Установщик и portable | не проверено | `setup.exe` без прав администратора, SmartScreen, WebView2; portable-zip; статус-точка агента зелёная | релиз v0.1.0 опубликован, SHA-256 сверены |

## Версии

| Компонент | Версия |
| --- | --- |
| Windows | Windows 11 Home Single Language 10.0.26200 (build 26200), Lenovo, 32 ГБ ОЗУ |
| Node.js | v26.4.0 (npm 11.17.0) |
| Rust (`rustc -vV`) | rustc 1.99.0 (b940084d7 2026-09-28), host x86_64-pc-windows-msvc, cargo 1.99.0 |
| Ollama | 0.40.4; `OLLAMA_MODELS=D:\AI\ollama\models`, `OLLAMA_CONTEXT_LENGTH=16384`, `OLLAMA_KV_CACHE_TYPE=q8_0`, `OLLAMA_FLASH_ATTENTION=1` |
| Модель (тег, `num_ctx`) | `qwen3-4b-32k:latest` (`num_ctx` 32768, из `~/.codewhale/config.toml` и `defaultModel` приложения); запасная `qwen2.5-coder:7b` |
| ComfyUI | 0.41.0 portable в `D:\AI\ComfyUI` (Python 3.13.14 встроенный, torch 2.14.0+cu130, CUDA 13.0, архитектуры до sm_120, RTX 5060 Laptop виден); чекпоинт `v1-5-pruned-emaonly.safetensors` (4 265 146 304 байт) в `F:\AI_Drive\AI_ComfyModels\checkpoints` |
| Godot | 4.8 (custom build, `F:\PROJECTS\godot-hd`) |
| CodeWhale (`codewhale --version`) | codewhale 0.10.1 (fead51eeea0f), `npm i -g codewhale` |
| GPU | NVIDIA GeForce RTX 5060 Laptop GPU, 8151 MiB, драйвер 610.62 |
| Python | 3.14.6 через `py`; sympy 1.14.0 |

## Что поменяно в коде (облако, 2026-10-16, ветка `claude/busy-darwin-juot7g`)

- Оверлей «Модели Ollama» показывает `ModelManager` (вкладки «Установленные» / «Загрузить» /
  «Запущенные») вместо старой `ModelsPanel`; `src/components/overlays/SidePanels.tsx`.
- Моки команд `models_*` в `e2e/mock/backend.ts` (`models_pull` шлёт прогресс в `Channel`
  страницы), фикстуры в `e2e/mock/defaults.ts`, 8 тестов `e2e/tests/models.spec.ts`;
  весь набор e2e — 41 тест, зелёный под Linux.
- `tools/check-shader.ps1` использует проект `codewhale-desktop/src-tauri/resources/godot-check`
  из репозитория (`-Godot`/`GODOT_BIN` для другого пути к Godot).
- `release.yml`: защита от публикации под уже существующим тегом, одна объединённая `--config`
  для версии и sidecar, ссылка на `INSTALL.md` по тегу, `GH_REPO` через env.
- Исследования синтезированы: `docs/research/codewhale-safety-design.md`,
  `docs/research/rtx5060-8gb-model-plan.md`, `docs/research/user-pdf-verification.md`.
- Документация: `docs/KNOWLEDGE_DRIVE.md` (раздел про диск `F:`), `PROJECT.md` (дорожная карта
  7.2–7.4, известные проблемы), `docs/HANDOFF_LOCAL_SESSION.md` (раздел 8).

## Что поменяно в коде (локальная сессия, 2026-10-16)

- `e9769c5` — `tools/pc-organize/`: `Invoke-PcAudit.ps1` (аудит ПК → `Desktop\PC-Audit\*.md/.json`),
  `Invoke-PcOrganize.ps1` (план сортировки, `-Apply`, журнал CSV, `-Undo`, ничего не удаляет),
  `Invoke-PcCleanup.ps1` (чистка пересоздаваемых кэшей), `README.md`; строка о наборе в `PROJECT.md`.
  Причина: раздел 0a хэндоффа — пользователь просил сначала навести порядок на ПК.
- `f02afbc` — `resources/drive/init-ai-drive.ps1`: ключ `-KeepOllamaModels` (скрипт больше не переписывает
  `OLLAMA_MODELS` без спроса) и лаунчер `py` для проверки SymPy (`python` в PATH здесь — алиас Store).
- Этот отчёт (`docs/LOCAL_RUN_REPORT.md`): разделы 0a–4, версии, раздел 8.

## Раздел 8: хвосты на машине пользователя

- `powershell -NoProfile -File tools\check-shader.ps1 Godot\WaterDemo\water.gdshader` →
  `COMPILES: Godot\WaterDemo\water.gdshader (11 uniforms)` — готово.
- `cargo clippy -- -D warnings` и `cargo test` в `src-tauri` на Windows — зелёные (47 тестов).
- e2e на Windows: `cd codewhale-desktop\e2e; npm ci; .\node_modules\.bin\playwright test` →
  **41 passed (36 s)** с переменной `CW_E2E_CHROMIUM` (см. «Проблемы и обходные пути»).

## Проверки из `docs/research/user-pdf-verification.md`, раздел 5 (на машине пользователя)

| № | Что проверялось | Результат 2026-10-17 |
| --- | --- | --- |
| 2 | `godot --headless --editor --script res://tool.gd` с `EditorScript._run()` на сборке 4.8 | **не работает**: `ERROR: Can't load the script "res://tool.gd" as it doesn't inherit from SceneTree or MainLoop`, код выхода 1. Сцены без окна строить только SceneTree-скриптом (как `water_demo.gd`) |
| 3 | Код возврата `godot --headless --check-only --script bad.gd` при синтаксической ошибке | **1** (`SCRIPT ERROR: Parse Error: Expected expression after "+" operator.`), 0.2 с; на корректном скрипте 0 |
| 4 | Скорость и загрузка `qwen3-4b-32k` | `ollama ps`: 5,3 ГБ, 100 % GPU, CONTEXT 32768; `ollama run --verbose`: eval 86,8 ток/с на коротком ответе, в ходе агента при контексте 13 тыс. токенов 44–46 ток/с; prompt eval 10 274 токена за 4,25 с |
| 1, 6 | MoE `qwen3:30b-a3b` на 8 ГБ; `LLAMA_ARG_FIT_TARGET` при открытом Godot | не проверялось: модель не скачана (18 ГБ), редактор Godot в сессии не открывался |

## Проблемы и обходные пути

- **Обрыв хода агента на длинной записи файла (найдено живым тестом раздела 5).** CodeWhale 0.10.1
  ждёт первые байты ответа модели `[stream] open_timeout_secs` = 45 с по умолчанию, а Ollama при
  вызове инструмента (`chat format: peg-native`) отдаёт весь вызов одним блоком в конце генерации.
  Запись `water.gdshader` на 1 249 токенов заняла 27,7 с и прошла; следующий шаг на ~1 950 токенов
  при 44 ток/с не укладывался в 45 с, клиент рвал соединение (в `server.log` Ollama восемь
  запросов ровно по 45,0 с со статусом 500 и `srv stop: cancel task`), три повтора «stream-open»
  переотправляли тот же промпт, и ход завершился со статусом failed без `godot_check`. Исправление:
  `[stream] open_timeout_secs = 300` в `~/.codewhale/config.toml` (поставлено; `codewhale config get
  stream` подтверждает), то же пишет приложение при настройке провайдера и передаёт
  `CODEWHALE_STREAM_OPEN_TIMEOUT_SECS=300` sidecar'у. Побочный вывод: для 4B-модели файлы лучше
  писать кусками (скиллы обновлены), а за один вызов инструмента не стоит просить больше ~1 500
  токенов.

- **Playwright e2e:** `npx playwright install chromium` в `codewhale-desktop\e2e` скачал сборку
  Chromium 1234 (npx подхватил другой Playwright), а `@playwright/test` 1.56.1 ждёт
  `chromium_headless_shell-1194` — все 41 тест падали на `browserType.launch`. Загрузка нужной
  сборки через `.\node_modules\.bin\playwright install chromium-headless-shell` на этой машине
  зависала на десятки минут (стояла блокировка `ms-playwright\__dirlock`, папка оставалась
  неполной). Рабочий обход: запускать тесты с уже скачанным Chromium через переменную из
  `playwright.config.ts`:
  `$env:CW_E2E_CHROMIUM='C:\Users\Stryker\AppData\Local\ms-playwright\chromium-1234\chrome-win64\chrome.exe'; .\node_modules\.bin\playwright test`
  → 41 passed. Когда сеть позволит, достаточно один раз выполнить
  `.\node_modules\.bin\playwright install chromium-headless-shell` и переменная не нужна.
- **Python:** команда `python` ведёт на алиас Microsoft Store («Python was not found»); рабочий
  интерпретатор — `py` (Python 3.14.6). `sympy` поставлен через `py -m pip install sympy`.
- **Репозиторий:** скачанный zip (`claude/optimistic-ride-hu7e74`) не был git-клоном; в папке
  сделан `git init` + `git remote add origin` + `git fetch` + `git checkout -f -B claude/busy-darwin-juot7g origin/…`.
- **Место на C:** 24 ГБ свободно из 476. Кандидаты на чистку без потерь (`Invoke-PcCleanup.ps1`):
  кэш шейдеров NVIDIA 33 ГБ, дампы WSL в Temp 19 ГБ, загрузки Unity Hub 4,6 ГБ, кэш npm 4,9 ГБ.
  Крупные решения пользователя: Fortnite 67 ГБ и UE 5.8 30 ГБ на C:, три редактора Unity 36 ГБ,
  образ Docker 54 ГБ (старая копия 53 ГБ ещё и на `F:\Docker`), hiberfil 13 ГБ.
