# Шаблоны ComfyUI для «фабрики ассетов»

Файлы `*_api.json` — рабочие графы ComfyUI в **API-формате** (ComfyUI → Settings → *Enable Dev
mode Options* → кнопка *Save (API Format)*), в которых значения заменены плейсхолдерами
`{{ИМЯ}}`. Приложение копирует их в `<диск>\AI_Workflows\`, а MCP-сервер `comfy` подставляет
параметры и ставит граф в очередь ComfyUI.

| Шаблон | Инструмент агента | Что нужно в ComfyUI |
| --- | --- | --- |
| `ui_icon_api.json` | `generate_ui_icon` | только стандартные ноды + любой SD 1.5 / SDXL чекпоинт |
| `pbr_material_api.json` | `generate_pbr_material` | стандартные ноды (только albedo); для normal/roughness добавьте ноды Marigold/Lotus и SaveImage с префиксами `pbr_normal`, `pbr_roughness` |
| `sprite_sheet_api.json` | `generate_sprite_sheet` | стандартные ноды; кадры = `batch_size`, лист собирает приложение |
| `prop3d_api.json` | `generate_3d_prop` | не поставляется: нужны кастомные ноды Hunyuan3D-2 / TRELLIS; сохраните свой граф с плейсхолдерами `{{POSITIVE_PROMPT}}`, `{{SEED}}` и выводом `.glb` |

## Плейсхолдеры

| Плейсхолдер | Тип | Кто задаёт |
| --- | --- | --- |
| `{{POSITIVE_PROMPT}}`, `{{NEGATIVE_PROMPT}}` | строка внутри текста | агент (negative — по умолчанию из инструмента) |
| `"{{SEED}}"` | число | случайный, если агент не задал |
| `"{{WIDTH}}"`, `"{{HEIGHT}}"` | число | агент / значение по умолчанию инструмента |
| `"{{STEPS}}"`, `"{{CFG}}"` | число | по умолчанию 20 и 6.5 |
| `"{{CHECKPOINT}}"` | строка | настройка `comfy.checkpoint` в приложении (имя файла из `models/checkpoints`) |
| `"{{FRAMES}}"` | число | `generate_sprite_sheet` |

Числовые плейсхолдеры пишутся **в кавычках** (`"seed": "{{SEED}}"`) — при подстановке кавычки
убираются, иначе ComfyUI получит строку вместо числа. Любой другой `{{ИМЯ}}` в шаблоне можно
передать через параметр `params` инструмента `comfy_generate`; незаполненные плейсхолдеры —
ошибка, а не тихий запуск.

Рядом с шаблоном можно положить `<имя>.meta.json`:
```json
{ "description": "Иконки предметов 512×512 с прозрачным фоном", "params": { "STYLE": { "default": "painterly" } } }
```
