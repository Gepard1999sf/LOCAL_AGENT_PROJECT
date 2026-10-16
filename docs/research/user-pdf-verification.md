# Верификация утверждений из PDF пользователя

PDF: «Автономные локальные ИИ-агенты: от аналога Claude Code до GameDev-пайплайна» (2026, 15 стр.).
Сам PDF в репозитории отсутствует; проверялись утверждения, перечисленные в
`docs/research/user-pdf-notes.md` (все, не только помеченные «(?)»). Дата проверки: 2026-10-16.
Железо пользователя: RTX 5060 8 ГБ, 32 ГБ ОЗУ, Windows, Godot 4.8 (`docs/research/user-hardware.md`).

Порядок проверки: сначала исследования проекта (`research-items.json`, `understand-*.md`),
затем код репозитория, затем первоисточники в сети (всего 17 обращений, 3 страницы не открылись по DNS:
docs.godotengine.org, jina.ai, nvidia.custhelp.com — для них взяты зеркала на GitHub либо
данные из `research-items.json` с указанием первоисточника).

## 1. Кратко

Проверено 41 утверждение: **подтверждено 22**, **частично 14**, **не подтверждено 2**,
**устарело 1**, **не удалось проверить 2**.

Пять самых важных поправок для проекта:

1. **`OLLAMA_VRAM_OVERHEAD` не существует.** Переменная называется `OLLAMA_GPU_OVERHEAD`
   («Reserve a portion of VRAM per GPU (bytes)», в байтах). Рядом с ней у Ollama ≥ 0.30 есть
   `LLAMA_ARG_FIT_TARGET` (запас VRAM на устройство в МиБ для авто-размещения llama.cpp) — именно
   её стоит поднимать, когда запущен редактор Godot. Обе серверные: на Windows вступают в силу
   только после выхода из трей-приложения и перезапуска.
2. **«Авто-балансировщик VRAM↔RAM, вычисляющий `num_gpu`» устарел.** Ollama ≥ 0.30 запускает
   upstream `llama-server` с `--fit on`: слои и эксперты MoE размещаются автоматически, а явный
   `num_gpu` отключает fit. Нужен не балансировщик, а один параметр запаса (`LLAMA_ARG_FIT_TARGET`
   / `OLLAMA_GPU_OVERHEAD`) и предупреждение, когда `/api/ps` показывает `size_vram < size`.
3. **32–48 tok/s для MoE 30B-A3B с экспертами на CPU на 8 ГБ не подтверждены.** Единственное
   измерение на маленькой карте в наших источниках — ~15 tok/s (RTX 2060 6 ГБ, Qwen3.5-35B-A3B
   Q4_K_M, `--n-cpu-moe 99`); цифры 31–53 tok/s относятся к 16 ГБ и названы автором оценками по
   пропускной способности, не замерами. Кроме того, Ollama не даёт `--cpu-moe`/`--override-tensor`
   на модель (issue #11772) — для ручного размещения экспертов нужен отдельный `llama-server`.
4. **Режимы CodeWhale — Plan / Work / Operate, а не Plan / Work / YOLO.** `Tab` переключает их по
   кругу при пустом композере; `yolo` — не режим, а устаревшая точка входа «Work + Full Access»;
   `Shift+Tab` переключает posture (Ask / Auto-Review / Full Access). `/undo`, `/restore` и
   side-git-снимки есть, но десктоп их не показывает — только `fork` (research #43).
5. **Мост в браузер не нужно писать самим.** `@playwright/mcp` уже умеет `--cdp-endpoint`
   (подключение к запущенному Chrome) и `--extension` (расширение Playwright MCP, использует
   залогиненные сессии пользователя). Context7 и Tavily/Brave — облачные сервисы с ключами
   (не «полностью локально»); из перечисленных самохостится только Jina Reader (Apache-2.0, Docker).

Дополнительно: `godot --headless --check-only --script` работает только для `.gd`; `.gdshader` так
не проверить — проект уже использует свой `check.gd` (SceneTree + `Shader.new()`). `temperature 0.15`
из примерного Modelfile для Qwen-семейства рискованна: низкая температура усиливает зацикливание
(research #73).

## 2. Таблица проверки

Вердикты: **✔ подтверждено**, **◐ частично**, **✘ не подтверждено**, **⌛ устарело**, **? не удалось проверить**.
`R#n` — элемент `docs/research/research-items.json` с индексом n (0-based).

### 2.1 Inference / VRAM

| Утверждение из PDF | Вердикт | Что на самом деле | Значение для проекта |
| --- | --- | --- | --- |
| `OLLAMA_FLASH_ATTENTION=1` | ✔ | FAQ Ollama: flash attention включается автоматически, где поддерживается; `=1` форсирует, `=0` выключает (`docs/faq.mdx`). | Не форсировать; оставить «auto», показывать значение из `server.log`. |
| `OLLAMA_KV_CACHE_TYPE=q8_0` | ✔ | FAQ: f16 по умолчанию, q8_0 «recommended if not using f16», ~½ памяти KV, требует flash attention; q4_0 ~¼ (`docs/faq.mdx`; R#15: с ротацией Адамара в llama.cpp q8_0 почти без потерь). Переменная серверная. | Рекомендация по умолчанию для 8 ГБ. Оценщик `models.rs:647-700` уже считает KV для q8_0/q4_0; `ModelManager.tsx:590` подсказывает переменную. Нужен «управляемый» запуск Ollama (R#19), иначе только инструкция пользователю. |
| `OLLAMA_KEEP_ALIVE=30m` | ✔ | Допустимые значения: duration («10m», «24h»), секунды, отрицательное = держать всегда, 0 = выгрузить сразу; по умолчанию 5m (`docs/faq.mdx`, `envconfig/config.go`). | 30m лучше, чем 5m. Для агентных сессий R#18 советует 2h или −1 плюс кнопка «Выгрузить» (`ollama_unload` уже есть, `ollama.rs:159`). Можно задать и без переменной: `POST /api/generate {model, keep_alive}` перед сессией. |
| `OLLAMA_NUM_PARALLEL=1` | ✔ | FAQ: «default 1»; память масштабируется как NUM_PARALLEL × CONTEXT_LENGTH. Для qwen3.5/3.6 и других гибридов Ollama сама форсирует 1 слот (R#22, `server/sched.go`). | Уже значение по умолчанию — ничего ставить не нужно; лишь не поднимать. Суб-агентов CodeWhale ограничить `[subagents] max_concurrent = 1`. |
| Ловушка контекста по умолчанию (2048/4096) → закреплять `num_ctx` | ✔ | Таблица Modelfile до сих пор пишет `num_ctx` «Default: 2048»; сервер же выбирает 4k для < 24 ГиБ VRAM, 32k для 24–48, 256k выше (`docs/context-length.mdx`, R#14). OpenAI-совместимый `/v1`, которым пользуется CodeWhale, не умеет передавать `num_ctx` (`understand-unused-capabilities.md` §1). | Уже решено вариантом модели: `ollama_create_variant` (`ollama.rs:135-156`) печёт `num_ctx` в `/api/create`. Осталось синхронизировать `context_window` в `config.toml` CodeWhale (иначе компакция считает 8192) — R#14, п. 4. |
| `OLLAMA_VRAM_OVERHEAD` резервирует VRAM под Godot/браузер (?) | ✘ | В `envconfig/config.go` такой переменной нет. Есть `OLLAMA_GPU_OVERHEAD` («Reserve a portion of VRAM per GPU (bytes)», по умолчанию 0) и `LLAMA_ARG_FIT_TARGET` («Target free VRAM margin per device for llama.cpp fit (MiB)», по умолчанию 1024). | Использовать правильные имена. При запущенном `godot*.exe` поднимать `LLAMA_ARG_FIT_TARGET` до ~2048 МиБ (R#16, R#23). |
| NVIDIA «CUDA - Sysmem Fallback Policy» → «Prefer No Sysmem Fallback» | ✔ | Настройка появилась в драйвере 546.01: NVIDIA Control Panel → Manage 3D Settings; выключает тихий сброс CUDA-аллокаций в общую память ценой риска падения при нехватке VRAM (R#23, первоисточник NVIDIA KB 5490 — сама страница в этой сессии недоступна). Можно задать для одной программы (ollama.exe). | Карточка «GPU health» с инструкцией (R#23) и детектор сброса по PDH «Shared GPU memory» (R#72). Ручной шаг, автоматизировать нельзя. |
| `num_thread` = число физических ядер | ◐ | Опция `num_thread` в API жива (`api/types.go`, `Runner.NumThread`), Ollama передаёт её в `llama-server --threads`, но из таблицы Modelfile её убрали — по умолчанию число потоков подбирается автоматически. Совет «физические ядра» — старая рекомендация из прежних версий документации. | Не выставлять вручную; актуально только при экспертах MoE на CPU, и то проверять замером. |
| Q4_K_M / Q5_K_M «для частей, выгружаемых на CPU» | ◐ | Q4_K_M остаётся стандартом (4.89 бит/вес); квантование применяется ко всей модели, а не к «частям на CPU». На RTX 50 (sm_120) llama.cpp использует FP4-ядра для NVFP4/MXFP4 — быстрее prefill (R#24). Q5_K_M на 8 ГБ почти всегда не помещается. | В выборе модели: сначала `-qat`/NVFP4-теги, затем Q4_K_M, IQ4_XS при нехватке; показывать `quantization_level` из `/api/show`. |
| MoE 30B-A3B: эксперты на CPU через `--override-tensor "\.ffn_.*_exps\.=CPU"` или `--cpu-moe`, attention+KV на GPU | ✔ | llama-server README: `-ot, --override-tensor <pattern>=<buffer type>`, `-cmoe, --cpu-moe` («keep all MoE weights in the CPU»), `-ncmoe, --n-cpu-moe N`. Но это флаги `llama-server`; Ollama их на модель не даёт (issue #11772), а с 0.30 её `--fit` сам переносит тензоры экспертов в ОЗУ при нехватке VRAM (R#16). | Через Ollama — ничего не настраивать, просто выбирать MoE-модель и смотреть `size_vram`/`size` в `/api/ps`. Ручное размещение — только в опциональном бэкенде `llama-server` (R#11/R#20, L). |
| …и даёт 32–48 tok/s (?) | ✘ | Измерено: RTX 2060 6 ГБ, Qwen3.5-35B-A3B Q4_K_M, `--n-cpu-moe 99`, KV q8_0, ctx 102k → ~15 tok/s генерации, 350–400 tok/s prefill (llama.cpp discussion #21112, R#16). 31–53 tok/s — оценки для 16 ГБ RTX 5060 Ti, автор называет их расчётом, не замером (R#3). Для 8 ГБ замеров нет. | Цифры в UI не обещать; добавить микробенчмарк (R#77) и показывать реальные `timings` из ответа. Ожидать «десятки», не 40+. |
| Авто-балансировщик VRAM↔RAM: `num_gpu` из свободной памяти `nvidia-smi` минус резерв | ⌛ | Ollama ≥ 0.30 собирает «minimal set, let llama-server auto-detect the rest» и передаёт `-ngl` только при явном `num_gpu`; явный `num_gpu` отключает `--fit` (R#16, `llm/llama_server.go`). | Не слать `num_gpu`. Вместо балансировщика — ползунок «Резерв VRAM для Godot/браузера» → `LLAMA_ARG_FIT_TARGET` и предупреждение «модель частично в ОЗУ» по `/api/ps`. |
| Prefix caching: `llama-server --cache-reuse` | ◐ | Флаг есть: `--cache-reuse N` («min chunk size to attempt reusing from the cache via KV shifting», по умолчанию 0); `cache_prompt` по умолчанию true — «только невиданный суффикс» пересчитывается. Ollama `--cache-reuse` не пробрасывает, но кэш префикса работает и с v0.33.3 отдаёт `prompt_eval_cached_count` (R#35). | Главный рычаг — неизменный префикс (см. 2.2), а не флаг. Показать «cache hit %» в StatusBar (R#18/R#35). `--cache-reuse` — только для бэкенда llama-server. |
| Спекулятивное декодирование с маленькой draft-моделью — выигрыш (?) | ◐ | Работает, но скромно и с условиями: RTX 4090, Qwen3.6-27B + одно-словарный драфт Qwen3.5-4B → 43.2 tok/s среднее против ~38 без драфта; кросс-словарные драфты «silently corrupt JSON & lists» (R#11). MTP-головы самой модели: 6 ГБ ноутбук, 35B-A3B 22.9 → 29.4 tok/s при n-max 2, но prefill падает и VRAM растёт (R#17). В Ollama включается параметром Modelfile `draft_num_predict` (0 = выкл.; для встроенного MTP обязателен). Старые флаги `--draft-max/--draft-min` в llama-server удалены (теперь `--spec-draft-n-max/-min`). | На 8 ГБ отдельный драфт не поместится рядом с основной моделью; пробовать только MTP-вариант (`draft_num_predict: 2`) с A/B-замером (R#17, S). Кросс-словарные драфты не предлагать никогда — ломают tool-calls. |

### 2.2 Agent workflow

| Утверждение из PDF | Вердикт | Что на самом деле | Значение для проекта |
| --- | --- | --- | --- |
| Двухмодельная маршрутизация: Plan (reasoning) → Work (быстрый кодер) | ◐ | Приём рабочий (Aider architect/editor: QwQ-32B + Qwen2.5-Coder 26.2% против 16.4% у одной модели, R#38), но на одном GPU 8 ГБ две модели одновременно не живут, а смена модели выбрасывает весь KV-кэш. Режимы CodeWhale модель не меняют. | Один и тот же тег, разный `reasoning_effort` (`think:true/false`) по фазам — поле есть на `POST /v1/threads` и `/turns`, десктоп его не шлёт (`understand-unused-capabilities.md` H2). Две модели — только если оценщик показывает, что обе влезают. |
| Режимы CodeWhale Plan / Work / YOLO, переключение `Tab` (?) | ◐ | `docs/MODES.md`: режимы **Plan** (shell и запись файлов отвергаются), **Work** (внутри `agent`, алиас `act`), **Operate**; `Tab` циклит Plan → Work → Operate при пустом композере, `Shift+Tab` — posture (Ask / Auto-Review / Full Access), `/mode <name>`, Alt+A/P/Y. `yolo` — «not a mode», устаревшая точка входа «Work + Full Access». Смена режима во время хода отклоняется. | Чип режима (Plan/Work) и чип posture в тулбаре; слать `mode` и `permission_posture` вместо клиентского авто-подтверждения (R#44). «YOLO» в UI не называть. |
| side-git снимки, `/undo`, `/restore` (?) перед автономными прогонами | ✔ | `MODES.md`: `/undo` возвращает файлы после отката Esc-Esc; `/restore` (и `/restore list [N]`) восстанавливает рабочее дерево из side-git снимков; инструмент `revert_turn`. Снимки лежат в `~/.codewhale/snapshots/<project_hash>/<worktree_hash>/.git`, `[snapshots].max_age_days` по умолчанию 7 (R#43). Runtime API: `patch-undo`, `file-revert`, `GET /v1/snapshots`, `POST /v1/snapshots/{id}/restore` (409 во время хода). | Десктоп вызывает только `fork` (`threads.ts`). Сделать меню отката на каждом ходе и вкладку «Снимки» (R#43, M). |
| «Нулевая мутация» префикса: первые ~4000 токенов (system + AGENTS.md + tools) неизменны | ✔ | llama-server `cache_prompt`: пересчитывается только невиданный суффикс; любая вставка в начало ломает кэш — Ollama v0.33.0 чинила именно это для Claude Code («tokens left» в начале промпта «broke the KV cache on every request», R#35). Число 4000 — иллюстративное, важна неизменность, не размер. | У CodeWhale есть `prompt_zones.rs` (PinnedPrefix/AppendLog), но «not yet wired» (R#35). Со стороны десктопа: не менять `allowed_tools`/skills между ходами, не класть время и git-статус в системный промпт, мерить cache-hit. |
| RepoMap через tree-sitter | ✔ | Приём Aider: tree-sitter извлекает определения/ссылки, персонализированный PageRank, бюджет `--map-tokens` 1k; RepoGraph (ICLR 2025) улучшил все 4 SWE-bench-метода (R#36). Есть `tree-sitter-gdscript` 6.1.0 и `tree-sitter-gdshader`. В CodeWhale/десктопе отсутствует. | R#36: `repo_map` как контекстный фрагмент после замороженного префикса, бюджет ~1k для 16k окон (M). |
| Долговременные заметки в SQLite/LanceDB через MCP | ◐ | У CodeWhale уже есть своя память: `GET/POST /v1/memory`, lens-ревью (`understand-unused-capabilities.md` §3). В проекте SQLite используется для FTS5-индекса `git_vault` (`index/git_vault.sqlite`), папка `vectordb` для LanceDB создаётся, но помечена «позже» (`docs/KNOWLEDGE_DRIVE.md`). | Не плодить второй слой памяти: сначала показать память CodeWhale в UI. Векторный поиск — опциональный индексатор (R#40, M, низкий приоритет). |
| Headless-самопроверка: `godot --headless --check-only --script <file.gd>` (?) после каждого сохранения | ✔ | Godot CLI docs: `--check-only` — «Only parse for errors and quit (use with --script)»; `--headless` — «Enable headless mode … Useful for servers and with --script»; опция «extended»: только в editor-сборках (или экспорт-шаблонах с `disable_path_overrides=false`). Для `.gd` проект делает ровно это (`drive_mcp.rs:1434-1500`, ветка `"gd"`: `--headless [--path <project>] --check-only --script <abs>`), ошибки парсит из вывода (`SCRIPT ERROR`, `Parse Error`), а не только по коду возврата (R#30 предупреждает, что код может быть 0). Для `.gdshader` этот путь не годится — используется `resources/godot-check/check.gd` (SceneTree + `Shader.new()`, ищет `SHADER ERROR`/`RESULT: LOADED`). «После каждого сохранения» сейчас держится на скилле `knowledge-drive` (п. 4) и послушности модели. | Сделать проверку детерминированной: gate в `verifier.rs` CodeWhale (до 12 пользовательских gate'ов) на `**/*.gd` и `**/*.gdshader` с вердиктом в результате инструмента `write/edit` (R#30, S). |
| `godot --headless --script tool.gd --quit` для сборки сцен EditorScript'ом | ? | Docs: скрипт для `--script` «must inherit from SceneTree or MainLoop»; `--quit` — «Quit after the first iteration». SceneTree-скрипт (как `check.gd`, он сам вызывает `quit()`) подтверждён. Запуск именно `EditorScript` без редактора документацией не описан (`EditorScript` выполняется в контексте редактора, т. е. потребуется `--editor`); в этой сессии не проверено. | Проверить на машине пользователя: `godot --headless --editor --script res://tool.gd` (ожидание — `_run()`), иначе строить сцены SceneTree-скриптом, как `Godot/WaterDemo/water_demo.gd`. |
| Шаблон AGENTS.md для Godot 4 (запрет Godot 3 API, статическая типизация, обязательная headless-проверка) | ✔ | CodeWhale читает `AGENTS.md` и показывает источники инструкций через `GET /v1/workspace/instructions` (R#51). В репозитории есть `AGENTS.md` именно такого вида (PowerShell 5.1, путь к Godot 4.8, обязательный `tools\check-shader.ps1`). | Правило «проверка после каждого шейдера» переносить из текста в gate (см. выше) — 4B-модель инструкции забывает. `tools\check-shader.ps1` теперь использует `resources\godot-check` из репозитория (исправлено 2026-10-16). |
| Библиотека скиллов `.agents/skills` (shaders_ref, fsm_template, scene_builder, daily_ops) | ✔ | Стандарт SKILL.md (agentskills.io); CodeWhale читает `.agents/skills` и `.claude/skills` как совместимые корни, пишет в `~/.codewhale/skills` и `<ws>/.codewhale/skills`; маршруты `/v1/skills*` (R#45). Проект ставит свои 3 скилла в `~/.codewhale/skills` или `.agents/skills` (`drive.rs:24, 548-567`). | Названных в PDF четырёх скиллов нет; `godot-shaders`, на который ссылается `AGENTS.md`, в репозитории тоже нет (живёт у пользователя). Добавить `shaders_ref`/`fsm_template`/`scene_builder` в `resources/skills` (S) и панель скиллов (R#45). |
| Пример Modelfile: `num_ctx 24576`, `temperature 0.15`, SYSTEM | ◐ | `PARAMETER num_ctx` и `temperature` (по умолчанию 0.8) и инструкция `SYSTEM` документированы (`docs/modelfile.mdx`). Но CodeWhale всегда шлёт свой системный промпт по `/v1/chat/completions`, поэтому `SYSTEM` из Modelfile вытесняется (на живой модели не проверено). Низкая температура у Qwen-семейства усиливает зацикливание (arXiv 2512.12895, R#73). | Вариант печь через `/api/create` (как сейчас, только `num_ctx`); `temperature` ниже ~0.6 не ставить; `SYSTEM` в Modelfile не писать. |

### 2.3 Web access

| Утверждение из PDF | Вердикт | Что на самом деле | Значение для проекта |
| --- | --- | --- | --- |
| Фильтр HTML → чистый Markdown + «токен-гард» ~12k символов / ~3k токенов | ◐ | Приём стандартный (так работают Jina Reader, Context7). В проекте лимит 12 000 символов уже стоит на ответах `ai-drive` (`drive_mcp.rs:266, 301, 374, 410`, `MAX_TOOL_TEXT`). Оценка «12k символов ≈ 3k токенов» занижена для кода и русского текста (ближе к 4–6k). Для веб-инструмента CodeWhale («Web», выключен в автоподтверждении `settings.rs:42`) формат выдачи не проверялся. | Лимит оставить, считать по токенам, а не символам, если появится счётчик. Веб-доступ подключать как MCP (ниже), а не доверять неизвестному формату встроенного инструмента. |
| Jina Reader: `r.jina.ai` / `s.jina.ai` | ✔ | `github.com/jina-ai/reader`: префикс `https://r.jina.ai/<url>` → LLM-friendly текст (по умолчанию plain text с `Title:`/`URL Source:`, markdown через заголовок `x-respond-with: markdown`); `https://s.jina.ai/<query>` — поиск, топ-5 результатов, `?site=` ограничивает домены. Apache-2.0, самохостится в Docker (`ghcr.io/jina-ai/reader:oss`). Анонимный трафик лимитируется сильнее всего, точных лимитов нет. | Самый дешёвый веб-доступ: MCP-обёртка/пресет над `r.jina.ai` с `max_chars` 12 000 (S). Самохост — только если нужна приватность URL. |
| Context7 MCP (`@upstash/context7-mcp`) | ✔ | Пакет есть; рекомендуемая установка `npx ctx7 setup` (OAuth, ключ, skill), удалённый сервер `https://mcp.context7.com/mcp` с Bearer-ключом. Инструменты: `resolve-library-id {libraryName, query}` и `query-docs {libraryId, query, language?}` (не `get-library-docs`). Ключ «recommended» (без него ниже лимиты). | Облачный, с ключом — не оффлайн. Для Godot/GLSL оффлайн-докcеты Zeal на диске знаний уже закрывают потребность; Context7 — пресет в менеджере MCP (R#46) для npm/Rust-библиотек. |
| Tavily / Brave MCP | ◐ | Существуют: референсный `@modelcontextprotocol/server-brave-search` (`BRAVE_API_KEY`), собственный сервер Brave (v2.1.4, 2026-09-17; web/local/video/image/news/summarizer), Tavily MCP от `tavily-ai` (search/extract/map/crawl). Все требуют API-ключ. Проверено по каталогам MCP, не по npm-реестру. | Пресеты в менеджере MCP; ключи — только через env-ссылки (CodeWhale `env_headers`/`bearer_token_env_var`, R#46). «Полностью локально» — нет. |
| SearXNG + Crawl4AI (полностью локально) | ? | В этой сессии не проверялось (оба — известные open-source проекты, но требуют Docker/Python-окружение и свою поддержку). | Низкий приоритет (L): для одного пользователя с диском знаний достаточно Jina + докcетов. |
| Браузерный мост: WebSocket `127.0.0.1:17420` + Chrome MV3-расширение (GET_ACTIVE_TAB, FETCH_URL в фоновой вкладке) | ◐ | Это проектное предложение, не факт. Готовый эквивалент уже есть: `@playwright/mcp --extension` — «Connect to a running browser instance (Edge/Chrome only). Requires the "Playwright Extension"», позволяет «leverage your logged-in sessions and browser state». Cookies остаются в Chrome, модели уходит только текст. | Не писать своё расширение и протокол. Пресет MCP `@playwright/mcp --extension` (S). |
| `@playwright/mcp --cdp-endpoint` к пользовательскому Chrome | ✔ | README playwright-mcp: `--cdp-endpoint <endpoint>` — «CDP endpoint to connect to» (env `PLAYWRIGHT_MCP_CDP_ENDPOINT`); в конфиге — «Chrome DevTools Protocol endpoint to connect to an existing browser instance». | Второй пресет (для Chrome, запущенного с `--remote-debugging-port`). |
| Сырой CDP на `:9222` | ✔ | Это именно тот endpoint, к которому подключается `--cdp-endpoint` (Chrome с `--remote-debugging-port=9222`). Свой CDP-клиент писать не нужно — его даёт playwright-mcp. | Не реализовывать вручную. Кнопки «Attach active tab» / «Send Chrome console errors» делать поверх MCP-инструментов Playwright или как dynamic tools десктопа (R#42, M). |

### 2.4 Reliability patterns

| Утверждение из PDF | Вердикт | Что на самом деле | Значение для проекта |
| --- | --- | --- | --- |
| Двухслойная память: read-only AGENTS.md + read-write `.agent_state/ledger.json` | ◐ | Пост Anthropic «Effective harnesses for long-running agents» описывает похожее: `claude-progress.txt` + список фич в JSON (JSON, потому что модель «less likely to inappropriately change or overwrite JSON files compared to Markdown»; менять можно только поле `passes`). Пара «AGENTS.md + ledger.json» — авторская адаптация. Реализовано: `ledger_record`/`ledger_list` (`drive_mcp.rs:1380-1414`), файл `<диск>\.agent_state\ledger.json` (`docs/FINAL_TEST_WATER.md`). | Готово. Полезно добавить правило «менять только статус» из поста и показывать ledger в панели диска. |
| Чтение логов по закладке (байтовые смещения в `bookmarks.json`) → только новые строки | ✔ | Реализовано: `log_read_since_bookmark` / `log_reset_bookmark` (`drive_mcp.rs:1310-1377`), сброс при перезаписи файла движком, перенос закладки только на выданный объём, тесты `bookmarks_advance_only_after_read` (`drive_mcp.rs:1863`). В посте Anthropic такого приёма нет (атрибуция ниже). | Готово. |
| Видимость неудачного чтения: `[SOURCE_UNREADABLE: reason]` вместо пустого вывода | ✔ | Реализовано во всех читающих инструментах (`drive_mcp.rs:603, 615, 699, 719, 721, 947, 1318, 1446`) плюс `[NO_NEW_EVENTS_SINCE_BOOKMARK]`; скилл `knowledge-drive` велит честно сообщать об этом. | Готово. |
| Лимит прогона `maxSteps` (например 10) → пауза `budget_reached` | ◐ | У CodeWhale есть `[tui] max_model_steps` (по умолчанию без лимита, 1–100000), `turn_wall_clock_secs`, env `CODEWHALE_MAX_MODEL_STEPS`, `CODEWHALE_TURN_WALL_CLOCK_SECS`, `CODEWHALE_STREAM_IDLE_TIMEOUT_SECS` (R#73). Десктоп их не задаёт; семантика «пауза с `budget_reached`» не проверялась. Значение 10 слишком мало для цикла «написал → проверил → поправил». | Раздел «Безопасность локальной модели»: `max_model_steps` ~150 и wall-clock, выставляемые при запуске sidecar (`runtime.rs`) — R#73, S. |
| Секреты вне модели (мост передаёт URL, cookies остаются в Chrome) | ✔ | Принцип соблюдён и в CodeWhale (MCP-менеджер хранит ключи только как ссылки на переменные окружения, R#46), и в `@playwright/mcp --extension` (сессии живут в Chrome). | Следовать при добавлении пресетов MCP: ключи только через env. |
| Подтверждать перед записью: DONE только после кода возврата 0 headless-проверки | ✔ | Реализовано: `ledger_record` вызывает `godot_check` и отказывает при `!check.ok` (`drive_mcp.rs:1380-1395`). У Anthropic то же: «Only mark features as "passing" after careful testing». Нюанс: для `.gd` `ok` требует и пустой список ошибок, и `exit_code == 0`; для шейдера — `RESULT: LOADED` (код возврата Godot при ошибках может быть 0). | Готово; не опираться только на код возврата. |
| Эти паттерны взяты из инженерных постов Anthropic | ◐ | Пост о harness'ах подтверждает: progress-файл, JSON-список с единственным изменяемым полем, `init.sh`, git-коммиты как точки восстановления, e2e-проверка до статуса «passing». В нём **нет** закладок, байтовых смещений, `[SOURCE_UNREADABLE]`, лимита шагов и темы секретов. | Атрибуцию в документах проекта смягчить: «по мотивам», а не «из постов Anthropic». |

### 2.5 Offline knowledge drive

| Утверждение из PDF | Вердикт | Что на самом деле | Значение для проекта |
| --- | --- | --- | --- |
| Диск 1 ТБ: модели, докcеты Zeal/Dash, `git_vault`, Kiwix ZIM (StackOverflow, gamedev.SE), векторы LanceDB; MCP-сервер поверх | ✔ | Реализовано: `drive.rs`, `drive_mcp.rs` (14 инструментов: `docsets_list/docset_search/docset_read`, `code_search/code_read`, `kiwix_books/kiwix_search/kiwix_read`, `math_eval`, `log_*`, `godot_check`, `ledger_*`), `resources/drive/init-ai-drive.ps1` (папки `models, docsets, git_vault\{gamedev,physics,fullstack}, kiwix_zim, math_physics, vectordb, bin, index`), `docs/KNOWLEDGE_DRIVE.md`. Сквозной тест с настоящим CodeWhale и Godot 4.7.2 (`docs/FINAL_TEST_WATER.md`). LanceDB — только папка, векторов нет. | Готово, кроме векторов (R#40, M, позже). Теги моделей в скрипте сверять с ollama.com перед скачиванием. |

## 3. Что уже реализовано в проекте из идей PDF

- **Ledger («подтверждать перед записью»)** — `codewhale-desktop/src-tauri/src/drive_mcp.rs:1380-1414` (`ledger_record`, `ledger_list`), файл `<диск>\.agent_state\ledger.json`; проверено в `docs/FINAL_TEST_WATER.md`.
- **Закладки логов и маркеры неудачного чтения** — `drive_mcp.rs:1310-1377`, `[SOURCE_UNREADABLE]`/`[NO_NEW_EVENTS_SINCE_BOOKMARK]`; правила для модели в `src-tauri/resources/skills/knowledge-drive/SKILL.md`.
- **Headless-проверка Godot** — `drive_mcp.rs:1434-1500` (`godot_check`: `.gd` → `--headless --check-only --script`, `.gdshader` → `--headless --path <check-project> --script res://check.gd -- <file>`), проект проверки `src-tauri/resources/godot-check/{check.gd,project.godot}`; пользовательский `tools/check-shader.ps1` использует тот же проект проверки (`-Godot`/`GODOT_BIN` задают путь к Godot).
- **Диск знаний и MCP-сервер `ai-drive`** — `src-tauri/src/drive.rs`, `drive_mcp.rs`, `resources/drive/init-ai-drive.ps1`, `resources/drive/sympy_helper.py`, панель `src/components/drive/`, документация `docs/KNOWLEDGE_DRIVE.md`.
- **Скиллы** — `src-tauri/resources/skills/{knowledge-drive,math-physics-godot,comfyui-gamedev}/SKILL.md`; установка в `~/.codewhale/skills` или `<проект>/.agents/skills` (`drive.rs:548-567`).
- **AGENTS.md под Godot 4** — корневой `AGENTS.md` (язык, PowerShell 5.1, путь к Godot 4.8, обязательная проверка шейдеров).
- **Эстафета VRAM с ComfyUI** — `src-tauri/src/comfy_mcp.rs:306-322` (`unload_ollama` через `/api/ps` + `keep_alive: 0`, `free_vram` через `POST /free`), запуск ComfyUI с `--lowvram --reserve-vram 1.0` (`src-tauri/src/comfy.rs:246`), `docs/COMFYUI.md`.
- **Автоподтверждение инструментов диска** — `src-tauri/src/settings.rs:33-43` (`ai_drive: true`), `src/hooks/useBootstrap.ts:62-66` (маска `mcp_ai-+drive_*`).
- **Токен-гард на ответах инструментов** — 12 000 символов по умолчанию (`drive_mcp.rs:266, 301, 374, 410`).
- **Закрепление `num_ctx` вариантом модели** — `src-tauri/src/ollama.rs:135-156` (`ollama_create_variant`, `/api/create` с `parameters.num_ctx`).
- **Оценка VRAM (веса + KV для f16/q8_0/q4_0 + запас)** — `src-tauri/src/models.rs:647-765`, подсказки про `OLLAMA_KV_CACHE_TYPE`/`OLLAMA_KEEP_ALIVE` в `src/components/models/ModelManager.tsx:590-591, 1051` (менеджер подключён в оверлей «Модели Ollama», e2e-тесты `e2e/tests/models.spec.ts`).
- **Телеметрия GPU** — `src-tauri/src/system.rs:16-30` (`nvidia-smi --query-gpu=…`).
- **Headless-рендер демо** — `Godot/WaterDemo`, `docs/final-test/water.png`.

## 4. Что стоит взять дальше

Идеи PDF, прошедшие проверку и ещё не реализованные (с ссылками на элементы исследования):

| Идея | Что делать | Усилия |
| --- | --- | --- |
| Детерминированная проверка после каждой записи `.gd`/`.gdshader` | Gate в `verifier.rs` CodeWhale по glob'ам, вердикт в результат `write/edit`, лимит 3 автопочинки на файл, бейдж COMPILES в карточке диффа (R#30). | S |
| Серверные настройки Ollama | «Управляемый» запуск `ollama serve` с профилем: `OLLAMA_KV_CACHE_TYPE=q8_0`, `OLLAMA_KEEP_ALIVE=2h`, `LLAMA_ARG_FIT_TARGET` (2048 МиБ при запущенном `godot*.exe`), `OLLAMA_GPU_OVERHEAD`; без этого — карточка-инструкция с перезапуском трея (R#19, R#23). | M |
| Откат вместо «YOLO» | Меню на каждом ходе: fork-at-turn / patch-undo / retry; вкладка «Снимки» (`GET /v1/snapshots`), 409 → тост (R#43). | M |
| Режим и posture вместо клиентского автоподтверждения | Чипы Plan/Work и Ask/Auto-Review/Full Access, поля `mode`, `permission_posture` на `POST /v1/threads` (R#44); переключатель `reasoning_effort` вместо второй модели (H2). | S |
| Лимиты прогона | `max_model_steps` (~150), `turn_wall_clock_secs`, `CODEWHALE_STREAM_IDLE_TIMEOUT_SECS` при запуске sidecar + детектор повторов (R#73). | S |
| Стабильный префикс и метрика кэша | Не менять состав инструментов/скиллов между ходами; показывать «cache hit % · tok/s» из `prompt_eval_cached_count`/`timings` (R#18, R#35). | S (метрика) / M (в CodeWhale) |
| Веб-доступ | Пресеты MCP: `r.jina.ai` с лимитом 12k символов, `@playwright/mcp --extension` и `--cdp-endpoint`, Context7/Brave/Tavily с ключами через env (R#46). Свой мост WS/MV3 не писать. | S |
| Кнопки «Прикрепить активную вкладку» / «Ошибки консоли Chrome» | Dynamic tools десктопа (`dynamic_tools` на ходе, результат через `tool-calls/{id}/result`) поверх Playwright MCP (R#42). | M |
| Детектор сброса VRAM в ОЗУ | PDH «Shared GPU memory» + карточка «Prefer No Sysmem Fallback» для ollama.exe (R#72, R#23). | M |
| Repo map | tree-sitter (GDScript/gdshader/Rust/TS) + PageRank, ~1k токенов после префикса (R#36). | M |
| Скиллы из PDF | `shaders_ref`, `fsm_template`, `scene_builder` в `resources/skills`; вернуть в репозиторий `godot-shaders`, на который ссылается `AGENTS.md`; панель скиллов (R#45). | S |
| Ускорение генерации | Только MTP-вариант (`draft_num_predict: 2`) с A/B-замером; отдельный драфт на 8 ГБ не предлагать (R#17). | S |
| MoE с ручным размещением экспертов | Опциональный бэкенд `llama-server` (`--fit`, `--n-cpu-moe`, `--cache-reuse`, n-gram спекуляция) — только если Ollama-режим не даёт нужной скорости (R#11, R#20, R#21). | L |
| Векторы LanceDB на диске | Фоновый индексатор с `qwen3-embedding:0.6b` через `/api/embed`, гибрид с ripgrep (R#40). | M (позже) |

Не брать:

- авто-балансировщик `num_gpu` (ломает `--fit`);
- `temperature 0.15` и `SYSTEM` в Modelfile (зацикливание; системный промпт всё равно свой у CodeWhale);
- собственное Chrome-расширение и протокол на `127.0.0.1:17420` (есть `@playwright/mcp --extension`);
- обещания «32–48 tok/s» в интерфейсе;
- SearXNG + Crawl4AI до появления реальной потребности;
- второй слой долговременной памяти (SQLite/LanceDB) поверх уже существующего `/v1/memory` CodeWhale.

## 5. Что проверить на машине пользователя (в облаке недоступно)

Эти пункты не закрыть без Windows, GPU и живой модели; удобно добавить их в локальную сессию
(`docs/HANDOFF_LOCAL_SESSION.md`, раздел 8):

1. Скорость MoE на 8 ГБ: `qwen3.6:35b-a3b` (или `qwen3:30b-a3b`) через Ollama с `num_ctx` 32k,
   `OLLAMA_KV_CACHE_TYPE=q8_0`; записать `eval_rate` из `ollama run --verbose` и долю `size_vram/size`
   из `/api/ps`. Это заменит неподтверждённые «32–48 tok/s» реальным числом.
2. `godot --headless --editor --script res://tool.gd` с `EditorScript`: выполняется ли `_run()` без
   окна на сборке 4.8 из `F:\PROJECTS\godot-hd`. Если нет — сцены строить SceneTree-скриптом.
3. Код возврата `godot --headless --check-only --script bad.gd` при синтаксической ошибке
   (ожидается ≠ 0; если 0 — `godot_check` всё равно поймает по `SCRIPT ERROR`/`Parse Error`, но это
   стоит зафиксировать).
4. Перекрывает ли системный промпт CodeWhale инструкцию `SYSTEM` из Modelfile при запросах через
   `/v1/chat/completions` (сравнить `ollama ps`/лог `server.log` с включённым `OLLAMA_DEBUG=1`).
5. Поведение CodeWhale при достижении `max_model_steps`: останавливается ли ход с понятным
   статусом (аналог `budget_reached` из PDF) и видно ли это в событиях `turn.lifecycle`.
6. Эффект `LLAMA_ARG_FIT_TARGET=2048` при открытом редакторе Godot: перестаёт ли модель спадать в
   общую память (столбец «Shared GPU memory» в Диспетчере задач) и сколько это стоит в tok/s.
7. После смены переменных окружения — обязательный выход из трей-приложения Ollama и перезапуск;
   проверить строку `server config env=map[…]` в `%LOCALAPPDATA%\Ollama\server.log`.

## 6. Источники

Первоисточники, открытые в этой сессии (2026-10-16):

- Ollama FAQ: https://raw.githubusercontent.com/ollama/ollama/main/docs/faq.mdx
- Ollama envconfig (`OLLAMA_GPU_OVERHEAD`, `LLAMA_ARG_FIT_TARGET`): https://raw.githubusercontent.com/ollama/ollama/main/envconfig/config.go
- Ollama Modelfile (`num_ctx`, `temperature`, `draft_num_predict`, `SYSTEM`): https://raw.githubusercontent.com/ollama/ollama/main/docs/modelfile.mdx
- Ollama API types (`num_thread`, `num_gpu`): https://raw.githubusercontent.com/ollama/ollama/main/api/types.go
- llama.cpp server README (`--override-tensor`, `--cpu-moe`, `--n-cpu-moe`, `--cache-reuse`, `cache_prompt`, `--fit`, `-ctk/-ctv`, spec-decoding): https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/README.md
- Godot CLI (`--headless`, `--check-only`, `--script`, `--quit`): https://raw.githubusercontent.com/godotengine/godot-docs/master/tutorials/editor/command_line_tutorial.rst (зеркало docs.godotengine.org)
- CodeWhale режимы, `/undo`, `/restore`, side-git: https://github.com/Hmbown/CodeWhale/blob/main/docs/MODES.md
- Jina Reader: https://github.com/jina-ai/reader
- Context7 MCP: https://github.com/upstash/context7
- Playwright MCP (`--cdp-endpoint`, `--extension`): https://github.com/microsoft/playwright-mcp
- Anthropic, «Effective harnesses for long-running agents»: https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
- Brave/Tavily MCP (вторичные каталоги): https://mcpservers.org/servers/arben-adm/brave-mcp-search, https://glama.ai/mcp/servers/@altinakseven/tavily-mcp-server

Не открылись (DNS): https://docs.godotengine.org/en/stable/tutorials/editor/command_line_tutorial.html,
https://jina.ai/reader/, https://nvidia.custhelp.com/app/answers/detail/a_id/5490 (данные NVIDIA взяты
из `research-items.json` #23, где эта страница цитируется).

Материалы проекта: `docs/research/research-items.json` (#2, 3, 11, 14–19, 21–24, 30, 35, 36, 38, 40,
42–46, 51, 72, 73, 77), `docs/research/understand-unused-capabilities.md`,
`docs/research/understand-ollama-api.md`, `docs/KNOWLEDGE_DRIVE.md`, `docs/COMFYUI.md`,
`docs/FINAL_TEST_WATER.md`, `docs/HANDOFF_LOCAL_SESSION.md`, код `codewhale-desktop/src-tauri/src/{drive_mcp,drive,comfy_mcp,comfy,ollama,models,settings,system}.rs`,
`codewhale-desktop/src/hooks/useBootstrap.ts`, `codewhale-desktop/src/lib/tools.ts`,
`codewhale-desktop/src-tauri/resources/{godot-check,skills,drive}`, `tools/check-shader.ps1`, `AGENTS.md`.
