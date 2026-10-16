---
name: comfyui-gamedev
description: Генерация игровых ассетов через ComfyUI по запечённым шаблонам — иконки UI, PBR-текстуры с материалом Godot, спрайт-листы, 3D-пропы — с эстафетой видеопамяти между Ollama и ComfyUI на 8 ГБ VRAM. Use when: иконка, текстура, спрайт, спрайт-лист, материал, PBR, albedo, normal map, ассет, картинка, сгенерируй изображение, ComfyUI, Stable Diffusion, SDXL, Flux, 3D-модель, glb.
invocation: model+user
---

# Фабрика ассетов (MCP-сервер `comfy`)

## Как устроен ComfyUI (что нужно знать, чтобы не ломать графы)
- ComfyUI исполняет **граф нод**: каждая нода — класс (`CheckpointLoaderSimple`, `KSampler`,
  `VAEDecode`, `SaveImage`…) с входами `inputs`. Связь — ссылка `["id_ноды", индекс_выхода]`.
- Есть два JSON-формата: **UI-формат** (с координатами, для редактора) и **API-формат**
  (`{"id": {"class_type", "inputs"}}`) — только второй принимает `POST /prompt`.
- Модели лежат в `models/checkpoints`, `models/loras`, `models/vae`, `models/controlnet`…;
  имя чекпоинта — имя файла. Кастомные ноды (`custom_nodes/`) добавляют новые `class_type`;
  если их нет, сервер вернёт `missing_node_type`.
- Результат кладётся в `output/`, его забирают через `GET /view?filename&subfolder&type`.
- **Не собирай графы с нуля.** Используй готовые шаблоны `*_api.json` с плейсхолдерами
  `{{POSITIVE_PROMPT}}`, `{{SEED}}`, `{{WIDTH}}`… — инструменты подставляют значения сами.

## Эстафета видеопамяти (8 ГБ VRAM + 32 ГБ RAM)
Языковая модель в Ollama и графическая модель в ComfyUI не помещаются в VRAM одновременно.
Инструменты генерации делают это автоматически: выгружают модель Ollama (`keep_alive: 0`) →
рендер в ComfyUI → `POST /free` (веса ComfyUI уходят в RAM) → управление возвращается тебе.
Поэтому **один вызов генерации = одна пауза ~5–20 с**; группируй запросы, не дёргай по кадру.

## Инструменты
| Задача | Инструмент | Ключевые параметры |
| --- | --- | --- |
| Проверить, что ComfyUI запущен и видит модели | `comfy_status` | — |
| Какие шаблоны и плейсхолдеры есть | `comfy_templates` | — |
| Проверить шаблон до запуска (нужные ноды установлены?) | `comfy_template_validate` | `template` |
| Иконка UI с чистым фоном | `generate_ui_icon` | `prompt`, `save_to` (файл .png в проекте), `size` (256/512) |
| PBR-материал (albedo [+normal/roughness если шаблон умеет]) + `.tres` для Godot | `generate_pbr_material` | `prompt`, `save_to` (папка/базовое_имя без расширения), `resolution` |
| Спрайт-лист для AnimatedSprite2D | `generate_sprite_sheet` | `prompt`, `frames`, `columns`, `frame_size`, `save_to` (.png) → ответ содержит `hframes`/`vframes` |
| 3D-проп (.glb) | `generate_3d_prop` | `prompt`, `save_to` (.glb); нужен шаблон `prop3d_api.json` с кастомными нодами |
| Любой свой шаблон | `comfy_generate` | `template`, `params` (объект плейсхолдер → значение), `save_to` |
| Найти ноду / её входы | `comfy_nodes_search`, `comfy_node_info` | `query` / `class` |
| Загрузить референс для img2img | `comfy_upload_image` | `path` |
| Освободить VRAM / прервать рендер | `comfy_free`, `comfy_interrupt` | — |

## Порядок работы
0. Инструменты сервера `comfy` отложенные: если их нет в списке — `tool_search` с запросом `comfy`.
1. `comfy_status`: ComfyUI должен быть `online`, иначе попроси пользователя нажать
   «Запустить ComfyUI» в панели «Фабрика ассетов».
2. Пиши промпт по формуле: *что это* + *стиль игры* + *ракурс* + *фон*. Для иконок: «centered,
   plain background»; для текстур: «seamless tileable, top-down»; для спрайтов: «side view,
   consistent character». Негативный промпт по умолчанию уже есть.
3. Сохраняй прямо в проект: иконки в `res://assets/ui/icons/`, текстуры в
   `res://assets/textures/`, спрайты в `res://assets/sprites/`. Параметр `save_to` — абсолютный
   путь или путь относительно папки проекта.
4. После генерации проверь, что файл появился (`read`/`list_dir`), и подключи его: материал
   `.tres` — в `MeshInstance3D.material_override`, спрайт-лист — `hframes`/`vframes` узла
   `Sprite2D`/`AnimatedSprite2D`.
5. Не нравится результат — меняй `seed` (параметр `seed`) и промпт, а не шаблон.
6. Шаблон падает с `missing_node_type` — скажи пользователю, каких нод не хватает (ответ
   `comfy_template_validate` их перечисляет); не пытайся «починить» JSON вручную.

## Ограничения
- Генерация занимает 5–60 с на 8 ГБ VRAM; таймаут инструмента 10 минут.
- `generate_3d_prop` и normal/roughness в `generate_pbr_material` работают только если
  пользователь собрал шаблоны с кастомными нодами (Hunyuan3D-2, Marigold/Lotus, RMBG).
- Прозрачный фон у иконок требует ноды удаления фона (RMBG) в шаблоне; базовый шаблон даёт
  однотонный фон — скажи об этом пользователю, если нужна прозрачность.
