---
name: knowledge-drive
description: Оффлайн-база знаний на съёмном диске через MCP-сервер ai-drive — точная документация (Zeal docsets), исходники эталонных репозиториев, дампы StackOverflow/StackExchange, символьная математика SymPy, чтение логов с закладки и проверка Godot перед записью задачи. Use when: Godot, GDScript, shader, gdshader, API сигнатура, как реализовать, формула, производная, градиент, нормаль, лог движка, проверить скрипт, задача выполнена.
invocation: model+user
---

# Диск знаний (MCP-сервер `ai-drive`)

## Когда использовать
Перед кодом, который опирается на API движка или библиотеки, которых ты не помнишь точно;
когда нужна эталонная реализация; когда нужна формула или её производная; после каждого
сохранения `.gd`/`.gdshader`; когда надо прочитать лог Godot или браузера.

Не вспоминай сигнатуры по памяти — ищи. Локальная модель ошибается в именах методов,
порядке аргументов и системе координат; диск отвечает за 2–25 мс и без интернета.

## Инструменты сервера `ai-drive`
| Задача | Инструмент | Как |
| --- | --- | --- |
| Точная сигнатура класса/метода | `docset_search` → `docset_read` | `docset_search {"query":"CharacterBody3D.move_and_slide"}`, затем `docset_read` по `path`; `docsets_list` показывает, что установлено |
| Эталонная реализация | `code_search` → `code_read` | `code_search {"query":"gerstner wave", "repo":"gamedev/godot-demo-projects"}`; читай только нужные строки `code_read {"path":..., "start_line":120, "end_line":180}` |
| Вопрос-ответ из StackOverflow / GameDev SE / Math SE | `kiwix_search` → `kiwix_read` | `kiwix_books` показывает доступные дампы; `kiwix_search {"query":"godot buoyancy rigidbody", "book":"gamedev.stackexchange.com_en_all"}` |
| Производная, градиент, интеграл, решение уравнения | `math_eval` | `math_eval {"mode":"gradient3d","expr":"sin(k*x - omega*t) + z^2"}` → готовое `Vector3(...)` для GDScript/GLSL |
| Новые строки лога без повторного чтения всего файла | `log_read_since_bookmark` | `{"source_key":"godot_log","path":"C:/Users/<user>/AppData/Roaming/Godot/app_userdata/<Проект>/logs/godot.log"}`; `log_reset_bookmark` начинает с начала |
| Проверка `.gd` / `.gdshader` без окна | `godot_check` | `{"path":"D:/Proj/water.gdshader"}` → `ok`, `errors`, `uniforms` |
| Отметить задачу выполненной | `ledger_record` | Записывает `VERIFIED_OK` только если `godot_check` прошёл; иначе возвращает ошибки |

## Порядок работы
0. Инструменты сервера `ai-drive` отложенные: если их нет в твоём списке инструментов, сначала
   вызови `tool_search` с запросом `ai-drive` (или `ai-drive godot_check`) — они подгрузятся как
   `mcp_ai--drive_<имя>`.
1. Сформулируй, какой факт нужен (имя метода, формула, пример). Один запрос — один факт.
2. Сначала `docset_search`, потом `code_search`, потом `kiwix_search`. Запросы короткие: 1–3 слова или идентификатор. `set_global*` ищет по префиксу.
3. Ответы обрезаются на 12 000 символах: сужай запрос или читай диапазон строк, не весь файл.
4. Написал или изменил `.gd` или `.gdshader` → сразу `godot_check`. Исправляй, пока `ok` не станет `true`.
5. Только после `ok: true` вызывай `ledger_record` и сообщай, что задача выполнена.
6. Логи читай через `log_read_since_bookmark` с постоянным `source_key` для каждого файла: так в контекст попадает только новое.

## Правила
- `[SOURCE_UNREADABLE: …]` или `[NO_NEW_EVENTS_SINCE_BOOKMARK]` — честно сообщи об этом; не выдумывай содержимое.
- «Индекс не построен» или «Kiwix недоступен» — попроси пользователя нажать в приложении «Диск знаний → Построить индекс / Запустить Kiwix».
- Godot 4: ось Y вверх, вперёд -Z; из Blender (Z вверх) координаты переводятся как `(x, y, z) → (x, z, -y)`.
- Не пиши в ledger без проверки. Не помечай задачу выполненной, если `godot_check` вернул ошибки.
