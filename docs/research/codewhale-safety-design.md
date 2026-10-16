# Модель безопасности CodeWhale Desktop: синтез исследования

Дата: 2026-10-16. Статус: предложение, не реализовано. Закрывает пункт «модель безопасности»
из `docs/HANDOFF_LOCAL_SESSION.md:191-192` и заготовку раздела 7.3 в `PROJECT.md:399-401`.

Все ссылки вида `файл:строки` относятся к корню репозитория. Ссылки `${SRC}/...` — на исходники
CodeWhale (0.10.2, коммит 182bafe) в том виде, в каком их цитируют записи
`docs/research/research-items.json`; сами исходники в этой сессии не перепроверялись, такие
утверждения помечены «по записи исследования». Там, где факт не подтверждён, стоит «не проверено».

## 1. Кратко

Сегодня агент на машине пользователя может всё, что может сам пользователь: `bash` CodeWhale
запускает PowerShell-команды без песочницы ОС (на Windows CodeWhale лишь кладёт потомков в Job
Object; по записи исследования, `${SRC}/docs/SANDBOX.md`), а единственный барьер — карточка
подтверждения в чате (`PROJECT.md:207-211`). Правила автоподтверждения живут на клиенте и
срабатывают по голому имени инструмента (`codewhale-desktop/src/hooks/useBootstrap.ts:49-70`),
то есть галочка «bash» означает «любая команда». Приложение не передаёт серверу ни `mode`, ни
`permission_posture`, ни `trust_mode` (`codewhale-desktop/src/store/app.ts:261-267`), и не
использует ни снимки рабочей папки, ни откат правок, ни хуки, ни политику разрешений, которые
Runtime API уже предоставляет (`docs/research/understand-unused-capabilities.md`, H6, H8).
Собственные MCP-серверы приложения (`ai-drive`, `comfy`) запускаются с правами пользователя и
частично принимают произвольные абсолютные пути. Предлагаемая модель переносит решения с клиента
на сервер (посадка `ask` / `auto_review` / `full_access`, типизированные правила, гранты на
сессию с отзывом), добавляет откат через side-git снимки и хуки-шлюзы, честно показывает
отсутствие песочницы и ограничивает пути в наших MCP-серверах. Настоящая изоляция команд на
Windows — отдельный долгий трек (restricted token / AppContainer / Windows Sandbox).

## 2. Модель угроз

### 2.1. Что агент может тронуть

| Актив | Как до него добирается агент | Где это видно |
|---|---|---|
| Файлы рабочей папки (`F:\PROJECTS\...`) | `read` / `write` / `edit` (автоподтверждение чтения включено по умолчанию), `bash` | `codewhale-desktop/src-tauri/src/settings.rs:33-44`, `codewhale-desktop/src/lib/tools.ts:69` |
| Файлы вне рабочей папки: профиль, `.ssh`, `.git-credentials`, другие проекты | `bash` (процесс пользователя, без песочницы); `read` — CodeWhale ограничивает ли чтение рабочей папкой, не проверено | `PROJECT.md:207-211`; запись «Windows command sandboxing gap» |
| История git проекта | `bash`: `git reset --hard`, `git push --force`, `git clean` | та же запись; хук-шаблон в записи «Hooks» |
| Диск знаний `D:\AI_Drive` | MCP `ai-drive`: чтение docsets/git_vault внутри корня через `contained()`; запись только `.agent_state/{bookmarks,ledger}.json` | `codewhale-desktop/src-tauri/src/drive_mcp.rs:145-147,161,705-730,945,1395` |
| Любой файл на диске (чтение) | `ai-drive` `log_read_since_bookmark` принимает произвольный абсолютный путь; `comfy_upload_image` читает любой путь и отправляет в ComfyUI | `drive_mcp.rs:1310-1320`; `codewhale-desktop/src-tauri/src/comfy_mcp.rs:826-828` |
| Любой файл на диске (запись) | MCP `comfy`: `save_to` — произвольный путь, `create_dir_all` + `fs::write` | `comfy_mcp.rs:366-384,417-420` |
| Процессы: Godot, Python, ComfyUI | `ai-drive` `godot_check` запускает `godot.exe` на любом пути; `math_eval` запускает `python sympy_helper.py <expr>` | `drive_mcp.rs:1436-1475,1277-1279` |
| Ollama (локальные модели) | `comfy` выгружает все загруженные модели (`keep_alive: 0`) перед рендером; `bash`: `ollama rm` | `comfy_mcp.rs:307-316`; выгрузка — осознанная настройка `unloadLlm` |
| Сеть | `bash` (`Invoke-WebRequest`, `curl`); инструмент `Web` отключён в профиле lite, но выключен только галочкой автоподтверждения, а не запретом | `tools.ts:69`, `settings.rs:42` |
| Токен Runtime API | Живёт только в Rust; webview ходит через `cw_request`, который пропускает лишь `/v1/*` и `/health` | `codewhale-desktop/src-tauri/src/api.rs:3-4,39-41` |

### 2.2. Кто противник

1. **Слабая локальная модель (4B–8B), которая ошибается.** Основной случай. Неверный путь,
   `Remove-Item -Recurse` не в той папке, затирание файла целиком вместо правки, бесконечные
   повторы одного вызова. Исследование по зацикливанию: distilled-модели зацикливаются заметно
   чаще учителей (arXiv 2512.12895, запись «Loop / hang watchdog»).
2. **Инъекция инструкций из прочитанного.** Источники: README и код чужих репозиториев в
   `git_vault`, страницы Kiwix/docsets, файлы сторонних аддонов в рабочей папке, `AGENTS.md`
   склонированного репозитория (CodeWhale загружает его как инструкции проекта, запись
   «AGENTS.md (AAIF standard)»), веб-страницы при включённом `Web`.
3. **Вредоносный скилл или MCP-сервер.** Скиллы из `.agents/skills` рабочей папки загружаются
   после доверия к папке; `allowed-tools` в SKILL.md «не даёт полномочий», CodeWhale лишь
   предупреждает (`${SRC}/docs/SKILLS.md:141-170`, по записи). MCP-реестр
   `registry.modelcontextprotocol.io` — preview с возможными ломающими изменениями (запись «MCP
   server manager»).
4. **Наши же MCP-серверы как усилитель ошибок модели.** Они не враждебны, но выполняются с
   правами пользователя и доверяют аргументам модели (п. 2.1).

### 2.3. Что вне рамок

- Злонамеренный пользователь или другой процесс с правами пользователя на той же машине: у него
  и так есть всё, что есть у агента.
- Сетевые атакующие: runtime слушает `127.0.0.1` с bearer-токеном
  (`codewhale-desktop/src-tauri/src/runtime.rs:308-312`); привязка Ollama к localhost не
  проверялась в этой работе.
- XSS и компрометация webview: есть CSP (`codewhale-desktop/src-tauri/tauri.conf.json:27-28`),
  глубже не рассматривается.
- Цепочка поставок бинарников CodeWhale, Ollama, Godot, ComfyUI.
- Многопользовательский режим и облачные провайдеры.

## 3. Что есть сейчас

| Механизм | Где в коде | Что защищает | Пробелы |
|---|---|---|---|
| Карточка подтверждения в чате: «Разрешить», «Отклонить», «Всегда в этой сессии» (`remember: true`) | `codewhale-desktop/src/components/chat/Blocks.tsx:205-262`; `app.ts:410-421` → `POST /v1/approvals/{id}` | Любой вызов, который CodeWhale счёл опасным, ждёт человека | «Всегда в этой сессии» создаёт серверный грант, но приложение не показывает список грантов и не умеет их отзывать (`approval_grants[]` из `GET /v1/threads/{id}` не читается: `codewhale-desktop/src/lib/types.ts:97-115`); истории решений (`GET /v1/approvals`) нет |
| Клиентское автоподтверждение по имени инструмента | `useBootstrap.ts:49-70`; правила `settings.rs:33-44`; UI `codewhale-desktop/src/components/overlays/SettingsPanel.tsx:245-258`; список `tools.ts:55-66` | Снимает рутину подтверждений чтения/поиска | Решение принимает клиент: работает только пока окно подписано на тред; «bash = любая команда»; сервер не знает о правилах. Замечание: в `understand-contract-threads.json` (notes, п. 1) автоподтверждение считалось мёртвым из-за формата `stream.progress`; в текущем коде это исправлено (`codewhale-desktop/src/store/threads.ts:234-237`) |
| Правило `ai_drive` включено по умолчанию | `settings.rs:43`; regex `/^mcp_ai-+drive_/` в `useBootstrap.ts:65`; обоснование `docs/KNOWLEDGE_DRIVE.md:61-64`, `docs/HANDOFF_LOCAL_SESSION.md:83` | Убирает подтверждение каждого поиска по диску знаний | CodeWhale спрашивает подтверждение на каждый инструмент пользовательского MCP-сервера, даже с `readOnlyHint` (`codewhale-desktop/src-tauri/src/mcp_core.rs:102-108` выставляет аннотации, сервер их игнорирует — наблюдение из `HANDOFF:195`). Правило покрывает и не-read-only инструменты: `ledger_record`, `log_reset_bookmark`, `godot_check` (запуск Godot), `math_eval` (запуск Python) |
| Облегчённый набор инструментов `LITE_TOOLS` | `tools.ts:69`; `app.ts:267,294` (`allowed_tools`) | Модель не видит `Web`, субагентов, фоновые команды | `bash` остаётся, а через него доступны сеть и любые файлы; `allowed_tools` — не политика, а список для модели |
| Прокси к Runtime API | `api.rs:39-41` (`allowed_path`), `api.rs:56-64` (bearer только в Rust) | Токен не попадает в webview; webview не может звать произвольные URL | `cw_request` не умеет свои заголовки, поэтому мутации MCP-серверов (`If-Match`) недоступны (`understand-unused-capabilities.md`, §5) |
| Tauri capabilities | `codewhale-desktop/src-tauri/capabilities/default.json`: `core:default`, `dialog:allow-open/save`, `opener:allow-open-url/default-urls/reveal-item-in-dir` | Webview не запускает процессы и не пишет файлы напрямую | Все опасные действия и так делает Rust по командам из `codewhale-desktop/src-tauri/src/lib.rs:37-85` (`comfy_start`, `drive_restart_ollama` с `taskkill`, `write_export`) |
| «Открыть в редакторе» и экспорт | `codewhale-desktop/src-tauri/src/system.rs:68-71` (список запрещённых расширений), `75-88` (только локальные диски, не UNC), `90-105` (только известные проекты), `218-226` (экспорт лишь `.md/.json/.txt`) | Имя файла от агента не запускает код и не утекает учётные данные по SMB | — |
| Запуск CodeWhale | `runtime.rs:308-321`: `app-server --http --host 127.0.0.1`, `CODEWHALE_RUNTIME_TOKEN`, `CODEWHALE_TELEMETRY=0`, cwd = рабочая папка | Локальный порт, случайный токен, телеметрия выключена | Не передаются `--approval-policy`, `--sandbox-mode`, `--set` (CLI их принимает: `${SRC}/crates/cli/src/lib.rs:91-165`, по `understand-unused-capabilities.md` §1); тред создаётся без `mode`/`permission_posture` (`app.ts:261-267`, `PROJECT.md:208-211`) |
| Регистрация MCP-серверов приложения | `codewhale-desktop/src-tauri/src/drive.rs:144-146,186-224,282-296`; `codewhale-desktop/src-tauri/src/comfy.rs:117-138,303-316` → `~/.codewhale/mcp.json` | Серверы — тот же exe (`mcp-drive`, `mcp-comfy`), окружение — только пути и URL, секретов нет | Нет `enabled_tools`: модели видны все 15 инструментов `ai-drive` и все инструменты `comfy` |
| Ограничение путей в `ai-drive` | `drive_mcp.rs:705-730` (`contained`: запрет `..`, canonicalize, `starts_with` корня) для `docset_read` (`:697`) и `code_read` (`:945`); `kiwix_read` принимает только путь с `/` без схемы (`:1068-1070`) | Чтение docsets/git_vault не выходит за корень диска | `log_read_since_bookmark` читает любой абсолютный путь (`:1310-1320`, до `max_chars*4` байт); `godot_check` запускает Godot на любом пути (`:1444-1445`); `math_eval` передаёт выражение в `sympy_helper.py` (`:1277-1279`) — безопасность `sympify` не проверена |
| Ограничение путей в `comfy` | нет | — | `save_to` — произвольный абсолютный путь (`comfy_mcp.rs:366-384`), запись `fs::write` (`:419-420`), `generate_pbr_material` переименовывает файлы и пишет `.tres` (`:788,794`); `comfy_upload_image` читает любой файл (`:826-828`) |
| Две памяти: AGENTS.md только для чтения + `.agent_state/ledger.json` для записи; «записать DONE только после проверки» | `drive_mcp.rs:1380-1405` (`ledger_record` отказывает, если `godot_check` не прошёл); `[SOURCE_UNREADABLE: …]` вместо пустого вывода (`:615,719-721`) | Реализует паттерны из `docs/research/user-pdf-notes.md` («Reliability patterns») | Леджер — часть диска знаний, не часть политики CodeWhale; ничего не мешает модели «отчитаться» текстом без записи |
| Side-git снимки (сервер) | Приложение лишь считает `changed_paths` из `turn.workspace_snapshot` (`threads.ts:277-283`) и обрабатывает `thread.forked` (`:240`); единственное действие — `forkThread` (`app.ts:357`) | Сейчас — ничего: отката нет (`PROJECT.md:207-208`) | Runtime даёт `POST /v1/threads/{id}/patch-undo {depth}`, `file-revert {path, snapshot_id, expected_hash}`, `GET /v1/snapshots`, `POST /v1/snapshots/{id}/restore` (409 при активном ходе), `[snapshots].max_age_days` по умолчанию 7 (`understand-unused-capabilities.md` H6; запись «Checkpoints / Rewind UI») |
| Хуки (сервер) | Не используются; `GET /v1/hooks` не вызывается | — | На тредах Runtime API срабатывают `tool_call_before` (allow/deny/ask, `updatedInput`, `additionalContext`), `shell_env`, `tool_call_after`, `on_error` (`${SRC}/docs/HOOKS.md:14-27,231-249`, по записи «Hooks») |
| Серверная политика (сервер) | Не используется | — | `permission_posture` = `ask` / `auto_review` / `full_access`, `mode` = `plan` / `agent` / `operate`, `trust_mode`, `allow_shell` (`understand-unused-capabilities.md` H8); 9-слойный порядок авторизации, `permissions.toml`, `[auto_review]` (запись «Server-side approval policy») |
| Песочница ОС (сервер) | нет | — | «The Windows command path currently reports no OS sandbox»; только Job Object с `KILL_ON_JOB_CLOSE` (`${SRC}/crates/tui/src/tools/shell.rs:37-40,700-704`, по записи «Windows command sandboxing gap») |

Вывод: защита сегодня — это подтверждение человеком плюс аккуратность Tauri-слоя. Всё, что
сервер умеет сверх этого, приложение не включает, а собственные MCP-серверы приложения ослабляют
границу «рабочая папка + диск знаний».

## 4. Предлагаемая модель

### 4.1. Уровни посадки (permission posture) в UI

Чип в тулбаре треда вместо карты галочек. Значения отправляются в `POST /v1/threads` и
`PATCH /v1/threads/{id}` (поля уже есть в типе `ThreadRecord`: `types.ts:22-25`).

| Уровень | Что уходит серверу | Для кого | Что показывает UI |
|---|---|---|---|
| **План** | `mode: "plan"`, `permission_posture: "ask"` | Разведка кодовой базы слабой моделью; «что бы ты сделал» | Значок «только чтение» |
| **Спрашивать** (по умолчанию) | `permission_posture: "ask"` + детерминированные правила чтения (п. 4.2) | Любая модель | Карточки подтверждения, список грантов |
| **Авто-ревью** | `permission_posture: "auto_review"` | Модели ≥ 14B; на одном GPU «страж» — это лишний вызов той же локальной модели, а 4B-модель — слабый судья (оговорка из записи «Server-side approval policy») | Предупреждение о стоимости и слабости судьи |
| **Полный доступ** | `permission_posture: "full_access"` | Только на один тред, после явного подтверждения; недоступен, пока горит чип «Нет песочницы ОС» без подтверждения (запись «Windows command sandboxing gap», short term) | Красная рамка треда |

Посадка хранится на сервере и действует даже когда окно не подписано на тред — это главное
преимущество перед `useBootstrap.ts:49-70`.

### 4.2. Что переезжает с клиента на сервер

1. **Галочки `read` / `grep_files` / `list_dir` / `file_search` → типизированные правила.**
   В `permissions.toml` (ранг User > Agent > BuiltinDefault, `${SRC}/docs/AUTHORIZATION_ORDER.md`,
   по записи) и/или `[auto_review]` allow-правила с `action_kind = "read"`
   (`${SRC}/docs/CONFIGURATION.md:2362-2407`, по записи). Не проверено: применяются ли
   `[auto_review]`-правила при посадке `ask` без вызова модели-стража.
2. **Галочка `bash` → семейства команд, а не инструмент целиком.** `remember: true` в
   `POST /v1/approvals/{id}` уже создаёт грант на семейство (например, `git status` с
   опциями без значений) или на точную команду (`${SRC}/docs/RUNTIME_API.md:1338-1407`, по
   записи). Галочку «bash» из настроек убрать; вместо неё — редактор правил: allow `git status`,
   `git diff`, `git log`; ask `git push`; deny `Remove-Item -Recurse` вне рабочей папки,
   `format`, `git push --force`.
3. **Правило `ai_drive` → серверное.** Два варианта, оба не проверены: (а) правило
   `permissions.toml` по имени `mcp_ai-drive_*` — неизвестно, поддерживает ли матчинг имена
   MCP-инструментов; (б) хук `tool_call_before`, который отвечает `{"decision":"allow"}` для
   белого списка read-only инструментов (`docset_*`, `code_*`, `kiwix_*`, `drive_status`,
   `ledger_list`) и молчит для остальных. Инструменты с побочными эффектами (`ledger_record`,
   `log_reset_bookmark`, `godot_check`, `math_eval`) из правила исключить.
4. **Гранты видимы и отзываемы.** Читать `approval_grants[]` из `GET /v1/threads/{id}`
   (`ThreadDetail`, `${SRC}/crates/tui/src/runtime_threads.rs:5304-5307`), слушать
   `approval.grant_added`, отзывать `DELETE /v1/threads/{id}/approval-grants/{grant_id}`.
5. **Журнал решений.** Панель на `GET /v1/approvals?limit=` с `decided_by`
   (`user` / `session_rule` / `posture` / `host`) — видно, кто именно пропустил вызов.
6. **Клиентское автоподтверждение остаётся только как запасной путь** на время перехода, затем
   удаляется вместе с ключом `autoApprove` (`settings.rs:33-44`).

### 4.3. Откат: снимки и rewind

- Меню на каждом сообщении пользователя (`codewhale-desktop/src/components/chat/Turn.tsx`):
  «Откатить разговор» (`fork-at-turn {turn_id}`, текст возвращается в композер), «Откатить
  разговор и файлы» (`patch-undo {depth}`, показать `files_restored`), «Повторить другой моделью»
  (`retry {depth, prompt}`). В `DiffView` — «Восстановить этот файл» (`file-revert`).
- Вкладка «Снимки» в инспекторе: `GET /v1/snapshots`, кнопка «Восстановить», заблокированная при
  активном ходе (сервер отвечает 409 и сначала делает `pre-restore:` снимок).
- Оговорки: `file-revert` требует `trust_mode` или Full Access (`${SRC}/crates/tui/src/runtime_api.rs:8230-8234`,
  по `understand-unused-capabilities.md` H6) — это конфликтует с посадкой «Спрашивать», нужно
  выяснить, распространяется ли требование на `patch-undo`. Не проверено, попадают ли в снимок
  файлы, изменённые командами `bash`, а не `edit`/`write` (у Claude Code — нет,
  https://code.claude.com/docs/en/checkpointing). Снимки старше `max_age_days` (7) исчезают —
  показать это в UI.

### 4.4. Хуки как шлюзы

Панель «Хуки» в настройках читает `GET /v1/hooks` (имя, событие, команда с маской, источник,
`problems`). Два шаблона на PowerShell через `cmd /C powershell -NoProfile -File ...` (на Windows
хуки запускаются через `cmd /C`, `${SRC}/docs/HOOKS.md:63-96`, по записи):

- `tool_call_before`-шлюз: deny для `Remove-Item -Recurse` вне рабочей папки, `format`,
  `git push --force`, чтения `.env`, `*.pem`, `id_rsa`, с `reason`, который модель прочитает.
  Выход с кодом 2 — жёсткий deny; строгий хук без вердикта закрывается отказом
  (`HOOKS.md:445-490`, по записи). Ответ ограничен 2000 символами на поле.
- `shell_env`: экспорт `GODOT_BIN=F:\PROJECTS\godot-hd\engine\bin\godot.windows.editor.x86_64.exe`,
  чтобы путь не жил в промптах и скиллах.

Предостережения: `default_timeout_secs` перекрывает таймауты отдельных хуков; события
`session_start` / `waiting_for_user` на тредах Runtime API не срабатывают. Хук — это лишний
процесс на каждый вызов инструмента; для PowerShell 5.1 это десятки–сотни миллисекунд, что на
фоне локальной генерации приемлемо (не измерялось).

### 4.5. Песочница на Windows: честно о реалистичном

Три горизонта из записи «Windows command sandboxing gap»:

1. **Сейчас (S).** Показать правду: чип «Нет песочницы ОС (Windows)» в строке состояния по
   `sandbox_mode` из `GET /v1/config` (`understand-unused-capabilities.md` §1; запись
   предлагает `codewhale doctor --json` с `sandbox_available` — не проверено). Пока чип горит,
   «Полный доступ» требует подтверждения, новые треды — «Спрашивать».
2. **Средний срок (M).** «Одноразовый запуск»: копия рабочей папки в Windows Sandbox через CLI
   `wsb start` / `wsb share --allow-write` / `wsb exec` (доступен с Windows 11 24H2,
   https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-cli).
   Только для сборки и тестов: GPU-редактор Godot там не запустить.
3. **Долгий срок (L, PR в CodeWhale).** Реальный `workspace-write` на Windows: либо переиспользовать
   LPAC AppContainer, в котором CodeWhale уже запускает плагины
   (`${SRC}/crates/tui/src/extension_host/supervisor.rs:49-54`, по записи; риск: dev-инструменты и
   GPU под LPAC могут не работать), либо подход Codex «restricted token + ACL с синтетическим SID»
   (Apache-2.0, https://github.com/openai/codex/tree/main/codex-rs/windows-sandbox-rs; открытый
   баг установки #52427 на 2026-10-09). Бэкенд `opensandbox` / Docker для цикла с Godot не
   подходит: Windows-бинарник Godot не запустится в Linux-контейнере.

### 4.6. Секреты

- Токен runtime остаётся в Rust (`api.rs:3-4`). Не менять.
- Учётные данные MCP-серверов — только через `bearer_token_env_var` / `env_headers`
  (`McpServerWriteRequest`, запись «MCP server manager»), никогда в `mcp.json` открытым текстом.
  Наши серверы и так получают только пути и URL (`drive.rs:190-210`, `comfy.rs:120-129`).
- Принцип из PDF пользователя «секреты вне модели» (`user-pdf-notes.md`, «Reliability patterns»):
  браузерный мост передаёт URL, cookies остаются в Chrome. Для файлов — deny-правило или хук на
  `.env`, `*.pem`, `id_rsa`, `%APPDATA%\...\Credentials`. Не проверено, поддерживает ли
  `permissions.toml` шаблоны путей; хук `tool_call_before` поддерживает точно.
- Инъекция через инструкции: вкладка «Контекст» с `GET /v1/workspace/instructions` показывает,
  какие `AGENTS.md` реально загружены (запись «AGENTS.md (AAIF standard)»), чтобы чужой репозиторий
  не подсовывал правила незаметно. Скиллы рабочей папки грузятся только после доверия к папке
  (`${SRC}/docs/SKILLS.md`, по записи) — показать это состояние в UI.

### 4.7. Сторожа: циклы, зависания, бюджеты

- **Бюджеты хода на сервере.** `[tui] max_model_steps` (1–100000, по умолчанию без лимита) и
  `turn_wall_clock_secs` (30–86400); env `CODEWHALE_MAX_MODEL_STEPS`,
  `CODEWHALE_TURN_WALL_CLOCK_SECS`, `CODEWHALE_STREAM_IDLE_TIMEOUT_SECS` (запись «Loop / hang
  watchdog»). Это и есть «run cap: maxSteps → budget_reached» из PDF пользователя. Выставлять
  при запуске сайдкара (`runtime.rs:308-321`) или через `--set` (не проверено, пишутся ли эти
  ключи через `POST /v1/config`).
- **Детектор циклов на клиенте** (`src/lib/watchdog.ts`, питается из `threads.ts`): пороги
  Gemini CLI — 5 одинаковых вызовов подряд, 10 повторов 50-символьного куска текста
  (https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/loopDetectionService.ts);
  OpenHands — `action_error = 3` (https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/conversation/stuck_detector.py).
  Реакция: немодальная полоса «Модель зациклилась» с кнопками Стоп (`.../interrupt`), Подтолкнуть,
  Повторить с anti-loop вариантом (`presence_penalty 1.5`, `repeat_penalty 1.1` через
  `ollama_create_variant`, так как CodeWhale параметры штрафов не передаёт).
- **Тихое усечение контекста.** Ollama по умолчанию обрезает историю, когда промпт больше
  `num_ctx` (`truncate`/`shift` = true при отсутствии; OpenAI-совместимый путь полей не имеет:
  https://github.com/ollama/ollama/blob/main/server/routes.go) и логирует это только на уровне
  debug. Модель, потерявшая результаты инструментов, начинает повторять действия — это тоже
  вопрос безопасности. Сравнивать `turn.usage.input_tokens` с `context_length` из `/api/ps` и
  показывать красный баннер выше ~95 % (запись «Context-overflow and CPU-offload guard»).

### 4.8. Меньше поверхности: инструменты, MCP, динамические инструменты

- Чем меньше инструментов видит 4B-модель, тем реже она выбирает не тот (Less-is-More,
  https://arxiv.org/abs/2411.15399). `LITE_TOOLS` уже это делает; добавить `enabled_tools` для
  `ai-drive` и `comfy` при регистрации (`drive.rs:186-224`, `comfy.rs:117-138`), оставив модели
  только нужные для текущей задачи.
- Динамические инструменты (`dynamic_tools` на `POST /v1/threads`, результат через
  `POST .../tool-calls/{call_id}/result`, запись «Client-executed dynamic tools») выполняются в
  Rust приложения: `godot.check_shader {path}` может проверять, что путь лежит в рабочей папке,
  до запуска Godot. Это безопаснее, чем `bash` + PowerShell-квотинг.
- Менеджер MCP с чек-листом инструментов и статусом подключения требует заголовка
  `If-Match: mcp-v1-<sha256>` на мутациях — нужно расширить `cw_request` (`api.rs:56-64`).

### 4.9. Собственные MCP-серверы приложения

Ограничить пути общим помощником `contained()` (`drive_mcp.rs:705-730`), вынесенным в
`mcp_core.rs`, с набором разрешённых корней: рабочая папка (передавать `WORKSPACE` в `env`
при регистрации), корень диска знаний, `AI_Workflows/output`, папка логов Godot.

| Инструмент | Сейчас | Предлагается |
|---|---|---|
| `comfy` `save_to` (`comfy_mcp.rs:366-384`) | любой путь | только рабочая папка или `<workflows_dir>/output`; иначе ошибка с подсказкой |
| `comfy_upload_image` (`:826-828`) | любой файл | только рабочая папка и диск знаний |
| `ai-drive` `log_read_since_bookmark` (`drive_mcp.rs:1310-1320`) | любой файл | рабочая папка, диск знаний, `%APPDATA%\Godot\app_userdata\*\logs` |
| `ai-drive` `godot_check` (`:1444-1445`) | любой путь | только рабочая папка и `GODOT_CHECK_PROJECT` |
| `ai-drive` `math_eval` (`:1277-1279`) | выражение → `sympy_helper.py` | проверить, что помощник использует `parse_expr` с ограниченными трансформациями, а не `sympify`/`eval` (не проверено) |

## 5. План внедрения

Трудоёмкость S/M/L — из записей `research-items.json`; дешёвое — первым.

| # | Шаг | Трудоёмкость | Файлы |
|---|---|---|---|
| 1 | Явная посадка: отправлять `mode` и `permission_posture: "ask"` при создании треда и в PATCH; чип посадки в тулбаре; чип «Нет песочницы ОС» по `GET /v1/config.sandbox_mode` | S | `codewhale-desktop/src/store/app.ts:261-267,286-294`; `src/components/layout/` (тулбар, строка состояния); `src/lib/types.ts:22-25` |
| 2 | Ограничение путей в своих MCP-серверах (п. 4.9) + передача `WORKSPACE` в `env` при регистрации + тесты на выход за корень | S | `src-tauri/src/mcp_core.rs`; `src-tauri/src/comfy_mcp.rs:366-384,826-828`; `src-tauri/src/drive_mcp.rs:1310-1320,1436-1446`; `src-tauri/src/drive.rs:186-224`; `src-tauri/src/comfy.rs:117-138` |
| 3 | Гранты и журнал: читать `approval_grants[]`, слушать `approval.grant_added`, кнопка «Отозвать» (`DELETE .../approval-grants/{id}`), панель `GET /v1/approvals` | S | `src/lib/types.ts:97-115`; `src/store/threads.ts:481-510`; `src/components/layout/Inspector.tsx` |
| 4 | Панель «Хуки» (`GET /v1/hooks`) + два PowerShell-шаблона (шлюз deny-листа, `shell_env` с `GODOT_BIN`); запись в `config.toml [[hooks.hooks]]` или `.codewhale/hooks.toml` проекта с последующим `POST /v1/config/reload` | S | `src/components/overlays/SettingsPanel.tsx`; `src-tauri/resources/hooks/*.ps1` (новые); `src-tauri/src/drive.rs` (по образцу `drive_install_skills`, `:548-570`) |
| 5 | Бюджеты и сторожа на сервере: раздел «Безопасность локальной модели» в настройках → `CODEWHALE_MAX_MODEL_STEPS`, `CODEWHALE_TURN_WALL_CLOCK_SECS`, `CODEWHALE_STREAM_IDLE_TIMEOUT_SECS` при запуске сайдкара | S | `src-tauri/src/runtime.rs:308-321`; `src-tauri/src/settings.rs:20-46`; `SettingsPanel.tsx` |
| 6 | `enabled_tools` для `ai-drive` / `comfy` при регистрации; исключить `ledger_record`, `godot_check`, `math_eval`, `log_reset_bookmark` из клиентского правила `ai_drive` до переноса на сервер | S | `src-tauri/src/drive.rs:186-224`; `src-tauri/src/comfy.rs:117-138`; `src/hooks/useBootstrap.ts:62-67` |
| 7 | Серверная политика вместо `autoApprove`: редактор правил `permissions.toml` и `[auto_review]`; посадка «Авто-ревью» только для ≥ 14B; удалить клиентское автоподтверждение | M | `src/hooks/useBootstrap.ts:49-70`; `src/lib/tools.ts:55-66`; `SettingsPanel.tsx:245-258`; `src-tauri/src/settings.rs:33-44`; Rust-редактор TOML + `POST /v1/config/reload` (ключи `POST /v1/config` ограничены, `understand-unused-capabilities.md` §1) |
| 8 | Rewind UI: меню на сообщении пользователя (`fork-at-turn`, `patch-undo`, `retry`), «Восстановить файл» в DiffView, вкладка «Снимки» с блокировкой при активном ходе (409 → тост) | M | `src/components/chat/Turn.tsx`; `src/components/chat/DiffView.tsx`; `Inspector.tsx`; `app.ts` (новые действия) |
| 9 | Детектор циклов и баннер усечения контекста на клиенте | M | `src/lib/watchdog.ts` (новый); `src/store/threads.ts`; `src-tauri/src/ollama.rs:35-66` (`/api/ps` → `context_length`, `size_vram`) |
| 10 | Менеджер MCP (статус, `enabled_tools`, Enable/Disable/Reconnect) — после поддержки `If-Match` в прокси | M | `src-tauri/src/api.rs:44-64`; новая вкладка в `SettingsPanel.tsx` |
| 11 | «Одноразовый запуск» в Windows Sandbox (`wsb`) для сборки и тестов | M | `src-tauri/src/system.rs` (новая команда); UI в композере |
| 12 | Настоящая изоляция `workspace-write` на Windows (LPAC или restricted token) — PR в CodeWhale | L | вне этого репозитория |

Шаги 1–6 не требуют изменений в CodeWhale и закрывают самые заметные дыры: невидимую посадку,
произвольные пути в наших серверах и неотзываемые гранты.

## 6. Открытые вопросы / не проверено

1. Поддерживает ли `permissions.toml` имена MCP-инструментов (`mcp_ai-drive_*`) и шаблоны путей
   файлов. Если нет — только хук `tool_call_before`.
2. Срабатывают ли `[auto_review]`-правила при посадке `ask` без вызова модели-стража, и какую
   модель использует страж на установке «только Ollama». Запись утверждает «одно stateless
   обращение к стражу, всё, что падает, закрывается отказом» (`${SRC}/docs/MODES.md:195-220`).
3. `codewhale doctor --json` с полем `sandbox_available` — из записи исследования, в исходниках
   не подтверждено. Есть `sandbox_mode` в `GET /v1/config`.
4. Какие ключи персистит `POST /v1/config` (`max_model_steps`, `turn_wall_clock_secs`?). Запасной
   путь — env при запуске или `--set KEY=VALUE`.
5. Попадают ли в side-git снимок файлы, изменённые через `bash`; требует ли `patch-undo`
   `trust_mode`/Full Access так же, как `file-revert` (`runtime_api.rs:8230-8234`).
6. Где именно живут глобальные хуки: `config.toml [[hooks.hooks]]` (по `HOOKS.md:63-96`) или
   отдельный файл; формат проектного `.codewhale/hooks.toml` и условие «trusted».
7. Можно ли заставить CodeWhale уважать `readOnlyHint` для пользовательских серверов или
   пропускать подтверждения через `enabled_tools`. Сейчас — наблюдение «спрашивает всегда»
   (`docs/KNOWLEDGE_DRIVE.md:61-64`).
8. Ограничивает ли встроенный `read` CodeWhale чтение рабочей папкой при `trust_mode = false`.
9. `sympy_helper.py`: `sympify`/`eval` или безопасный парсер.
10. Для `.gd` `godot --headless --check-only` может возвращать код 0 при ошибках — разбирать
    stderr (утверждение из записи «Deterministic verify-on-edit loop», источник
    https://skills.sh/sethcarney/custom-plugins/godot-cli, не проверено).
11. Привязка Ollama к `127.0.0.1` (`OLLAMA_HOST`) на машине пользователя.
12. Стоимость хука на PowerShell 5.1 на каждый вызов инструмента — не измерялась.

## 7. Источники

Записи `docs/research/research-items.json` (и копии в `research-now.json`, ключ
`safety-ecosystem`): «Server-side approval policy», «Windows command sandboxing gap»,
«Checkpoints / Rewind UI», «Hooks: read-only panel», «MCP server manager in the desktop»,
«Agent Skills (SKILL.md open standard)», «AGENTS.md (AAIF standard)», «Client-executed dynamic
tools», «Lean tool surface», «Loop / hang watchdog», «Context-overflow and CPU-offload guard»,
«Deterministic verify-on-edit loop». Их внешние источники:

- https://github.com/Hmbown/CodeWhale/blob/main/docs/AUTHORIZATION_ORDER.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/MODES.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/RUNTIME_API.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/CONFIGURATION.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/HOOKS.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/SANDBOX.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/SKILLS.md
- https://github.com/Hmbown/CodeWhale/blob/main/docs/MCP.md
- https://www.anthropic.com/engineering/claude-code-auto-mode
- https://code.claude.com/docs/en/checkpointing
- https://openai.com/index/building-codex-windows-sandbox/
- https://github.com/openai/codex/tree/main/codex-rs/windows-sandbox-rs
- https://github.com/openai/codex/issues/52427
- https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-cli
- https://raw.githubusercontent.com/agentskills/agentskills/main/docs/specification.mdx
- https://github.com/agentsmd/agents.md
- https://github.com/modelcontextprotocol/registry
- https://arxiv.org/abs/2411.15399
- https://arxiv.org/abs/2512.12895
- https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/loopDetectionService.ts
- https://github.com/OpenHands/software-agent-sdk/blob/main/openhands-sdk/openhands/sdk/conversation/stuck_detector.py
- https://github.com/ollama/ollama/blob/main/server/routes.go
- https://github.com/ollama/ollama/blob/main/server/prompt.go
- https://github.com/ollama/ollama/blob/main/docs/modelfile.mdx
- https://skills.sh/sethcarney/custom-plugins/godot-cli

Внутренние документы: `docs/research/understand-unused-capabilities.md` (§1, §5, H6, H8, H10),
`docs/research/understand-contract-threads.json` (notes), `docs/research/user-pdf-notes.md`
(«Agent workflow», «Reliability patterns»), `docs/KNOWLEDGE_DRIVE.md:61-71`,
`docs/HANDOFF_LOCAL_SESSION.md:83,191-197`, `PROJECT.md:31-32,193-211,399-401`.
