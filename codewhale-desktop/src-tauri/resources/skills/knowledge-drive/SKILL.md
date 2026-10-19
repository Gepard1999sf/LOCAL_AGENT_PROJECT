---
name: knowledge-drive
description: Оффлайн-база знаний ai-drive (mcp_ai--drive_*) — документация Godot, эталонный код, StackExchange, SymPy, логи, проверка Godot перед записью задачи. Use when: Godot, GDScript, shader, gdshader, API, как реализовать, формула, производная, нормаль, лог, проверить скрипт, godot_check, ledger_record, задача выполнена.
invocation: model+user
---

# Диск знаний `ai-drive`

## Главное (вывод обрезан: нужное здесь и в конце)
1. Файлы — в КОРЕНЬ проекта (папка с `project.godot`, обычно рабочая): `res://water.gdshader` = `<рабочая папка>/water.gdshader`. Папки `project/`, `res/` не создавай.
2. `.gdshader`: `write` скелета из конца → `mcp_ai--drive_godot_check {"path":"<рабочая папка>/water.gdshader"}` → правка → снова `godot_check`.
3. В ошибке `(:N)` — номер строки, `Подсказка:`/`Hint:` — что делать. Исправь ровно строку N. Один `edit` — одно изменение.
4. `edit` не прошёл 2 раза → `read` файла, потом `write` всего файла с переписанной функцией.
5. «Готово» — только если последний `godot_check` вернул `ok: true`, затем `mcp_ai--drive_ledger_record`. Иначе назови оставшуюся ошибку.
6. Пиши коротко: 1–2 предложения перед вызовом, без пересказов и повторов.
7. `mcp_ai--drive_*` (два дефиса) уже в списке. Нет нужного — `tool_search {"query":"mcp_ai-drive_docset_read"}` (точное имя, один дефис).
8. Скрытое: `retrieve_tool_result {"ref":"<ref из omitted>","mode":"lines","lines":"23-37"}` (вызовы), потом `"39-64"` (шейдеры).

## Инструменты
| Задача | Вызов |
| --- | --- |
| Пример шейдера | `mcp_ai--drive_code_search {"query":"shader_type spatial water"}` или `mcp_ai--drive_code_read {"path":"gamedev/godot-demo-projects/compute/texture/water_plane/water_shader.gdshader"}` |
| Справка по шейдерам | `mcp_ai--drive_code_read {"path":"gamedev/godot-docs/tutorials/shaders/shader_reference/spatial_shader.rst","start_line":203,"end_line":360}`: вершины 203–359, фрагмент 360–541; ещё `shading_language.rst` |
| Сигнатура класса/метода | `mcp_ai--drive_docset_search {"query":"CharacterBody3D.move_and_slide","docset":"Godot"}` → `mcp_ai--drive_docset_read {"docset":"Godot","path":...}` из ответа |
| Эталонный код | `mcp_ai--drive_code_search {"query":"gerstner wave"}` → `mcp_ai--drive_code_read {"path":..., "start_line":120, "end_line":180}` |
| StackOverflow / GameDev SE / Math SE | `mcp_ai--drive_kiwix_search {"query":"godot buoyancy rigidbody"}` → `mcp_ai--drive_kiwix_read {"path":...}` |
| Производная, градиент, интеграл, уравнение | `mcp_ai--drive_math_eval {"mode":"gradient3d","expr":"sin(k*x - omega*t) + z^2"}` → готовое `Vector3(...)` |
| Проверка `.gd` / `.gdshader` | `mcp_ai--drive_godot_check {"path":"D:/Proj/water.gdshader"}` → `ok`, `errors`, `uniforms` |
| Задача выполнена | `mcp_ai--drive_ledger_record {"task_id":"water","script_path":"D:/Proj/water.gdshader","summary":"..."}` — только после `ok: true` |
| Новые строки лога | `mcp_ai--drive_log_read_since_bookmark {"source_key":"godot_log","path":".../logs/godot.log"}` |

Нет инструмента в списке — `tool_search` с точными именами через пробел, с одним дефисом:
`{"query":"mcp_ai-drive_docset_read mcp_ai-drive_log_read_since_bookmark"}`; или вызови полным именем `mcp_ai--drive_<имя>`.

## Порядок работы
1. Один запрос — один факт. Шейдер: сначала пример (`code_search`/`code_read`), потом справка. API: `docset_search`
   с `"docset":"Godot"`, потом `code_search`, потом `kiwix_search`. Запросы короткие: 1–3 слова или идентификатор.
2. Ответы обрезаются на 12 000 символах: сужай запрос или читай диапазон строк, не весь файл.
3. Написал или изменил `.gd`/`.gdshader` → сразу `mcp_ai--drive_godot_check`. Исправляй, пока `ok` не станет `true`.
4. Только после `ok: true` — `mcp_ai--drive_ledger_record`, потом сообщи, что задача выполнена.
5. Логи — через `log_read_since_bookmark` с постоянным `source_key`: в контекст попадает только новое.
6. В `godot_check` и `ledger_record` — абсолютный путь: рабочая папка + имя файла, без подпапок. `write` на 150+ строк
   идёт десятки секунд и может оборвать ход: сначала скелет до 80 строк, потом `edit` по одному изменению.
7. `edit`: `oldText` — 1–5 строк, скопированных из последнего `read`; новый текст должен отличаться от старого.

## `.gdshader` — язык шейдеров Godot, не GLSL
- Первая строка `shader_type spatial;`, при необходимости вторая — `render_mode ...;`. Без неё `godot_check` пишет
  `Shader type  not supported` — допиши `shader_type spatial;` первой строкой.
- `vertex()`: `VERTEX`, `NORMAL`, `UV`, `TIME`. `fragment()`: `ALBEDO`, `NORMAL`, `ROUGHNESS`, `METALLIC`, `SPECULAR`, `ALPHA`, `VIEW`, `FRAGCOORD`. Текстуры `texture()`/`textureLod()`, производные только `dFdx`/`dFdy`.
- Цвет в spatial — только `ALBEDO = vec3(...)`; `COLOR` в `fragment()` только читается: «Constants cannot be modified».
- Типы не смешивай: `UV` — `vec2`; `vec3 p = UV` → «Invalid assignment of 'vec2' to 'vec3'», пиши `vec3(UV, 0.0)` или `vec3(d.x, 0.0, d.y)`.
- Нет и не компилируется: `gl_FragColor`, `gl_FragData`, `gl_Position`, `dFdz`, `texture2D`, `#version`, `main()`, `attribute`/`in`/`out`.
- Числа с точкой: `2.0 * x`, `pow(x, 2.0)`; `int * float` — ошибка. `PI` встроена — не объявляй её («Redefinition of 'PI'»).
- Глобальные переменные — только `uniform`, `const` или `varying` («Global non-constant variables are not supported»).

## Правила
- `[SOURCE_UNREADABLE: …]` или `[NO_NEW_EVENTS_SINCE_BOOKMARK]` — честно сообщи об этом; не выдумывай содержимое.
- «Индекс не построен» / «Kiwix недоступен» — попроси пользователя нажать «Диск знаний → Построить индекс / Запустить Kiwix».
- Godot 4: Y вверх, вперёд -Z; из Blender (Z вверх): `(x, y, z) → (x, z, -y)`.
- Не помечай задачу выполненной, если `godot_check` вернул ошибки.

## Скелет воды — компилируется (3 волны Герстнера, нормаль, пена, френель)
```glsl
shader_type spatial;
uniform vec4 wave_a = vec4(1.0, 0.6, 0.25, 12.0);
uniform vec4 wave_b = vec4(-0.4, 1.0, 0.2, 7.0);
uniform vec4 wave_c = vec4(0.8, -0.3, 0.15, 4.0);
varying float crest;

vec3 gerstner(vec4 w, vec3 p, inout vec3 t, inout vec3 b) {
	float k = 2.0 * PI / w.w;
	vec2 d = normalize(w.xy);
	float f = k * (dot(d, p.xz) - sqrt(9.8 / k) * TIME);
	float s = w.z * sin(f);
	float c = w.z * cos(f);
	t += vec3(-d.x * d.x * s, d.x * c, -d.x * d.y * s);
	b += vec3(-d.x * d.y * s, d.y * c, -d.y * d.y * s);
	return vec3(d.x * c, s, d.y * c) / k;
}

void vertex() {
	vec3 t = vec3(1.0, 0.0, 0.0);
	vec3 b = vec3(0.0, 0.0, 1.0);
	vec3 o = gerstner(wave_a, VERTEX, t, b) + gerstner(wave_b, VERTEX, t, b) + gerstner(wave_c, VERTEX, t, b);
	crest = o.y;
	VERTEX += o;
	NORMAL = normalize(cross(b, t));
}

void fragment() {
	float fresnel = pow(1.0 - clamp(dot(NORMAL, VIEW), 0.0, 1.0), 3.0);
	float foam = smoothstep(0.2, 0.4, crest);
	ALBEDO = mix(mix(vec3(0.02, 0.18, 0.4), vec3(0.2, 0.75, 0.85), fresnel), vec3(1.0), foam);
	ROUGHNESS = mix(0.05, 0.6, foam);
}
```
