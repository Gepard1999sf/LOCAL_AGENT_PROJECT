# Отчёт о локальном прогоне (Windows, машина пользователя)

Заполняется локальной сессией по `docs/HANDOFF_LOCAL_SESSION.md`, раздел 9. Строки со статусом
«не проверено» — то, что требует Windows, GPU, живой модели, Godot и ComfyUI и в облачном
контейнере (Linux) выполнить нельзя. Колонка «Проверено в облаке 2026-10-16» — что уже
подтверждено без машины пользователя.

## Разделы 1–7

| Раздел | Статус | Почему / что проверить | Проверено в облаке 2026-10-16 |
| --- | --- | --- | --- |
| 0a. Порядок на ПК (аудит, сортировка) | не проверено | `tools\pc-organize\Invoke-PcAudit.ps1`, затем `Invoke-PcOrganize.ps1` (пробный прогон → `-Apply`) | скрипты проверены парсером и самотестом под PowerShell 7 в Linux |
| 1. Проверка окружения | не проверено | нужны версии Node/Rust/Ollama/Python/Godot/WebView2/nvidia-smi с машины | — |
| 2. Сборка и первый запуск | не проверено | `npm run tauri dev`, окно, модель в заголовке, ответ в чате | `npx tsc --noEmit`, `npm run build` (Vite), `cargo clippy -- -D warnings`, `cargo test` (47 тестов) — зелёные под Linux |
| 3. Настройки приложения | не проверено | модель по умолчанию с инструментами и `num_ctx` ≥ 16k, папка диска, путь к Godot, автоподтверждение ai-drive | — |
| 4. Диск знаний | не проверено | подготовка, наполнение (`init-ai-drive.ps1`), индекс, Kiwix, скиллы, MCP, `mcp-drive status` / `godot_check` → `"ok": true, "uniforms": 11` | — |
| 5. Финальный тест: шейдер воды | не проверено | настоящая модель пишет `water.gdshader`, `godot_check`, `ledger_record`, рендер `D:\water.png` | сквозной тест с `mock_ollama.py` (`docs/FINAL_TEST_WATER.md`) |
| 6. ComfyUI: фабрика ассетов | не проверено | `comfy_template_validate`, `generate_ui_icon`, индикатор VRAM, `freedVram: true` | 17/17 против стенда `mock_comfy.py` |
| 7. Установщик и portable | не проверено | `setup.exe` без прав администратора, SmartScreen, WebView2; portable-zip; статус-точка агента зелёная | релиз v0.1.0 опубликован, SHA-256 сверены |

## Версии

| Компонент | Версия |
| --- | --- |
| Windows | _(заполнить)_ |
| Node.js | _(заполнить)_ |
| Rust (`rustc -vV`) | _(заполнить)_ |
| Ollama | _(заполнить)_ |
| Модель (тег, `num_ctx`) | _(заполнить)_ |
| ComfyUI | _(заполнить)_ |
| Godot | 4.8 (custom build, `F:\PROJECTS\godot-hd`) |
| CodeWhale (`codewhale --version`) | _(заполнить)_ |

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

## Что поменяно в коде (локальная сессия)

_(заполнить: коммиты, файлы, причины)_

## Проблемы и обходные пути

_(заполнить: какая модель, что пошло не так, что помогло)_
