# План моделей ComfyUI для фабрики ассетов (RTX 5060 Laptop 8 ГБ)

Дата: 2026-10-19. Машина: RTX 5060 Laptop 8 ГБ (Blackwell, sm_120: FP8 и NVFP4), 32 ГБ ОЗУ,
ComfyUI 0.41.0 portable (`D:\AI\ComfyUI`, torch 2.14.0+cu130, comfy_kitchen 0.2.37, dynamic VRAM),
модели — `F:\AI_Drive\AI_ComfyModels` через `extra_model_paths.yaml`. Сейчас там только SD 1.5,
и результат пользователю не нравится.

Как готовился план: размеры файлов, лицензии и признак gated сняты через HF API
(`/api/models/<repo>?blobs=true`). Ссылки проверены HEAD-запросом. Локальная установка ComfyUI
прочитана, но не изменялась. GPU не использовался, ничего не скачивалось. Все цифры скорости
ниже — **оценки (не проверено)**, если рядом не указан замер. Их нужно снять на GPU-этапе.

## 1. Итог

| Роль | Выбор | Скачать | Лицензия |
| --- | --- | --- | --- |
| 1. Быстрая модель (иконки, спрайты, albedo) | **Z-Image-Turbo NVFP4** + TE `qwen_3_4b_fp8_mixed` + VAE `ae` | 10,48 ГБ | Apache-2.0 |
| 2. Качественная модель (hero-арт, текстуры 2K) | **HiDream-O1-Image fp8_scaled** (один файл, TE встроен) | 8,07 ГБ | MIT |
| 3. Удаление фона | **BiRefNet** (репак Comfy-Org, ноды ядра) | 0,44 ГБ | MIT |
| 4. Апскейл | **RealESRGAN x4plus** + **x4plus_anime_6B** (быстро), **SeedVR2 3B int8** (качество); пиксель-арт — без модели | 4,04 ГБ | BSD-3 / Apache-2.0 |
| 5. Normal из albedo | **GLSL Sobel в ноде ядра `GLSLShader`** (без модели) | 0 | — |
| 6. LoRA для быстрой модели | **Pixel Art Style (tarn59)** и **Icons Redmond ZIT** | 0,26 ГБ | Apache-2.0 |
| 7. Image-to-3D (по желанию, с проверкой) | **TRELLIS.2 int8** (ноды ядра) | 8,51 ГБ | MIT (+ DINOv3, см. § 6) |
| **Всего** | | **31,80 ГБ** (без 3D — 23,29 ГБ) | |

Главное:
- **Кастомные ноды не нужны.** Всё, что выбрано, работает на нодах ядра ComfyUI 0.41.0:
  `UNETLoader`, `CLIPLoader(lumina2)`, `ModelSamplingAuraFlow`, `LoraLoaderModelOnly`,
  `LoadBackgroundRemovalModel`/`RemoveBackground`, `UpscaleModelLoader`, `SeedVR2*`, `GLSLShader`,
  `ImageQuantize`, `Trellis2*`, `SaveGLB`. Pip-пакеты ставить не надо. `comfy_angle` и `pyopengl`
  для `GLSLShader` уже стоят в `python_embeded`.
- Все файлы лежат на Hugging Face/GitHub **без gate и без токена**.
- Все лицензии разрешают коммерческое использование результатов (оговорка про DINOv3 — в § 6).
- Старые шаблоны на `CheckpointLoaderSimple` и SD 1.5 под новые модели не годятся. Нужны новые
  графы `*_api.json` (§ 8). Есть и пара мелких правок в коде приложения, они на потом (§ 9).

Почему так, а не иначе:
- **Z-Image-Turbo** — Elo 934 на арене Artificial Analysis, лучший результат среди Apache-моделей,
  которые целиком помещаются в 8 ГБ. Ему хватает 8 шагов при CFG 1. NVFP4 (4,51 ГБ) — родной
  формат тензорных ядер Blackwell. Он помещается в VRAM целиком и оставляет запас под батч кадров.
  INT8 (6,2 ГБ) — запасной вариант, если NVFP4 даст артефакты (§ 10).
- **HiDream-O1-Image** — Elo 974, выше Z-Image. Лицензия MIT, без gate. Это **один** чекпоинт
  на 8,07 ГБ: работает в пиксельном пространстве, отдельного TE и VAE нет. Модель понимает
  разрешение до 2048×2048, то есть подходит для текстур 2K. Медленная (40 шагов, CFG 5) — для
  роли «минуты на картинку» это приемлемо.
- Почему отклонены другие:
  - **FLUX 3 Image Klein** (15.10.2026): gated, лицензия FLUX Kommunity, независимой оценки пока нет.
  - **Qwen-Image-2.1** (Elo 1042): лицензия некоммерческая.
  - **Ming-Image-0.1-Design** (Elo 999, MIT): TE весит 12,8–19,5 ГБ. Вместе с DiT это ~19 ГБ ОЗУ —
    на 32 ГБ, когда WSL/Docker держат ~13 ГБ, начнётся своп.
  - **Krea 2 Turbo**: NVFP4 на 7,67 ГБ, в 8 ГБ не влезает; есть порог выручки.
  - **SDXL Lightning**: работает со старыми шаблонами, но по качеству и следованию промпту заметно
    слабее Z-Image.

## 2. Файлы: URL → путь на диске

Пути абсолютные. Папки `clip` и `unet` ComfyUI сам маппит в `text_encoders` и `diffusion_models`
(`map_legacy`, `folder_paths.py:112`), поэтому TE лежит в `clip` — ключ `clip: clip` в yaml уже есть.

| # | Файл | URL | ГБ | Куда |
| --- | --- | --- | --- | --- |
| 1a | z_image_turbo_nvfp4.safetensors | https://huggingface.co/Comfy-Org/z_image_turbo/resolve/main/split_files/diffusion_models/z_image_turbo_nvfp4.safetensors | 4,510 | `F:\AI_Drive\AI_ComfyModels\diffusion_models\z_image_turbo_nvfp4.safetensors` |
| 1b | qwen_3_4b_fp8_mixed.safetensors | https://huggingface.co/Comfy-Org/z_image_turbo/resolve/main/split_files/text_encoders/qwen_3_4b_fp8_mixed.safetensors | 5,632 | `F:\AI_Drive\AI_ComfyModels\clip\qwen_3_4b_fp8_mixed.safetensors` |
| 1c | ae.safetensors | https://huggingface.co/Comfy-Org/z_image_turbo/resolve/main/split_files/vae/ae.safetensors | 0,335 | `F:\AI_Drive\AI_ComfyModels\vae\ae.safetensors` |
| 2 | hidream_o1_image_fp8_scaled.safetensors | https://huggingface.co/Comfy-Org/HiDream-O1-Image/resolve/main/checkpoints/hidream_o1_image_fp8_scaled.safetensors | 8,068 | `F:\AI_Drive\AI_ComfyModels\checkpoints\hidream_o1_image_fp8_scaled.safetensors` |
| 3 | birefnet.safetensors | https://huggingface.co/Comfy-Org/BiRefNet/resolve/main/background_removal/birefnet.safetensors | 0,444 | `F:\AI_Drive\AI_ComfyModels\background_removal\birefnet.safetensors` (**новая папка и ключ yaml**, § 3) |
| 4a | RealESRGAN_x4plus.safetensors | https://huggingface.co/Comfy-Org/Real-ESRGAN_repackaged/resolve/main/RealESRGAN_x4plus.safetensors | 0,067 | `F:\AI_Drive\AI_ComfyModels\upscale_models\RealESRGAN_x4plus.safetensors` |
| 4b | RealESRGAN_x4plus_anime_6B.pth | https://github.com/xinntao/Real-ESRGAN/releases/download/v0.2.2.4/RealESRGAN_x4plus_anime_6B.pth | 0,018 | `F:\AI_Drive\AI_ComfyModels\upscale_models\RealESRGAN_x4plus_anime_6B.pth` |
| 4c | seedvr2_3b_int8_convrot.safetensors | https://huggingface.co/Comfy-Org/SeedVR2/resolve/main/diffusion_models/seedvr2_3b_int8_convrot.safetensors | 3,458 | `F:\AI_Drive\AI_ComfyModels\diffusion_models\seedvr2_3b_int8_convrot.safetensors` |
| 4d | seedvr2_ema_vae_fp16.safetensors | https://huggingface.co/Comfy-Org/SeedVR2/resolve/main/vae/seedvr2_ema_vae_fp16.safetensors | 0,501 | `F:\AI_Drive\AI_ComfyModels\vae\seedvr2_ema_vae_fp16.safetensors` |
| 6a | pixel_art_style_z_image_turbo.safetensors | https://huggingface.co/tarn59/pixel_art_style_lora_z_image_turbo/resolve/main/pixel_art_style_z_image_turbo.safetensors | 0,170 | `F:\AI_Drive\AI_ComfyModels\loras\pixel_art_style_z_image_turbo.safetensors` |
| 6b | [ZImage.Turbo]Icons_Redmond.safetensors | https://huggingface.co/artificialguybr/ICONS-REDMOND-ZIMAGETURBO/resolve/main/%5BZImage.Turbo%5DIcons_Redmond.safetensors | 0,085 | `F:\AI_Drive\AI_ComfyModels\loras\icons_redmond_zimage_turbo.safetensors` (переименован: квадратные скобки в PowerShell — wildcard) |
| 7a | trellis_2_int8_convrot.safetensors | https://huggingface.co/Comfy-Org/TRELLIS.2/resolve/main/diffusion_models/trellis_2_int8_convrot.safetensors | 5,253 | `F:\AI_Drive\AI_ComfyModels\diffusion_models\trellis_2_int8_convrot.safetensors` |
| 7b | dino_v3_vit_l.safetensors | https://huggingface.co/Comfy-Org/TRELLIS.2/resolve/main/clip_vision/dino_v3_vit_l.safetensors | 1,213 | `F:\AI_Drive\AI_ComfyModels\clip_vision\dino_v3_vit_l.safetensors` |
| 7c | trellis_2_shape_vae_bf16.safetensors | https://huggingface.co/Comfy-Org/TRELLIS.2/resolve/main/vae/trellis_2_shape_vae_bf16.safetensors | 1,096 | `F:\AI_Drive\AI_ComfyModels\vae\trellis_2_shape_vae_bf16.safetensors` |
| 7d | trellis_2_texture_vae_bf16.safetensors | https://huggingface.co/Comfy-Org/TRELLIS.2/resolve/main/vae/trellis_2_texture_vae_bf16.safetensors | 0,948 | `F:\AI_Drive\AI_ComfyModels\vae\trellis_2_texture_vae_bf16.safetensors` |

Очерёдность скачивания (каждый шаг сразу даёт результат):
1. **P1** — 1a–1c, 3, 4a, 4b, 6a, 6b, всего ≈ 11,3 ГБ. Хватает для иконок, спрайтов и PBR (§ 8).
2. **P2** — 2 (8,07 ГБ). Hero-арт и текстуры 2K.
3. **P3** — 4c, 4d (3,96 ГБ). Апскейл с восстановлением деталей.
4. **P4** — 7a–7d (8,51 ГБ). Скачивать, только когда 2D-конвейер проверен на GPU. Если TRELLIS.2
   не уложится в 8 ГБ (§ 7), удалить.

Запас по P5, в итог не входит:
- `z_image_turbo_int8_convrot` (6,20 ГБ) — если NVFP4 плохо держит качество или LoRA.
- FLUX.2 klein 4B NVFP4 (2,46 ГБ, Apache): https://huggingface.co/black-forest-labs/FLUX.2-klein-4b-nvfp4
  и `flux2-vae` (0,34 ГБ). Нужен для правок по референсу: вариации готовой иконки, кадры одного
  персонажа.

Скачивание на GPU-этапе (Git Bash, в фоне, с докачкой). F: — это USB T7, места свободно 619 ГБ:
```bash
dl() { mkdir -p "$(dirname "$2")"; curl -L --fail --retry 5 --retry-delay 10 -C - -o "$2.part" "$1" && mv -f "$2.part" "$2"; }
M=/f/AI_Drive/AI_ComfyModels
dl https://huggingface.co/Comfy-Org/z_image_turbo/resolve/main/split_files/diffusion_models/z_image_turbo_nvfp4.safetensors "$M/diffusion_models/z_image_turbo_nvfp4.safetensors"
dl "https://huggingface.co/artificialguybr/ICONS-REDMOND-ZIMAGETURBO/resolve/main/%5BZImage.Turbo%5DIcons_Redmond.safetensors" "$M/loras/icons_redmond_zimage_turbo.safetensors"
# … остальные строки таблицы так же; затем сверить размеры с HF API (?blobs=true → siblings[].size)
```

## 3. Что поправить в `extra_model_paths.yaml` и почему правка слетит

Ядро 0.41 ищет модель BiRefNet в папке `background_removal` (`folder_paths.py:59`). В yaml
приложения такого ключа нет. В блок `ai_drive:` нужно добавить строку и создать папку:
```yaml
  background_removal: background_removal
```
Необязательно: `vae_approx: vae_approx` с `taef1_decoder.pth` (https://github.com/madebyollin/taesd).
Тогда живой предпросмотр TAESD для Z-Image будет в цвете, а не через latent2rgb.

Ловушка: «Установить шаблоны» (`comfy_install_templates`, `codewhale-desktop/src-tauri/src/comfy.rs:356-362`)
каждый раз **перезаписывает** yaml по списку `MODEL_FOLDERS` (`comfy.rs:21`). Ручная правка
пропадёт при следующей установке. Постоянное решение — добавить `"background_removal"`
в `MODEL_FOLDERS` (§ 9). Пока этого нет, правку yaml придётся повторять после каждой установки.

Скорость диска: `docs/COMFYUI.md` фиксирует, что чтение с F: проседает до 37 МБ/с, когда диск
занят. Холодная загрузка набора Z-Image (10,5 ГБ) с F: в таком режиме — ~4,7 мин, с NVMe —
секунды. GPU-этапу стоит скопировать рабочий набор P1 в `D:\AI\ComfyUI\ComfyUI\models\...`
(свободно 392 ГБ). ComfyUI ищет там первым — так уже сделано с SD 1.5. Канонические копии
остаются на F:.

## 4. Скорость и качество (ожидания)

| Операция | Ожидание после прогрева | Основание |
| --- | --- | --- |
| Z-Image-Turbo NVFP4, 1024², 8 шагов | ~8–15 с | оценка. Ориентиры: ≤30 с, часто ~13 с на 8 ГБ с FP8 (zimageturbo.org); 15–20 с на RTX 4060 FP8 (dev.to). NVFP4 на Blackwell — нижняя часть диапазона. Не проверено |
| Спрайты: батч 8 кадров по 768² | ~40–80 с | оценка ×(768/1024)² на кадр. Не проверено |
| HiDream-O1 fp8, 1536², 40 шагов, CFG 5 | ~2–4 мин (2048² — до ~6 мин) | оценка. 8 ГБ весов > свободной VRAM, часть слоёв идёт потоком из ОЗУ. Замеров на 8 ГБ нет. Не проверено |
| BiRefNet, 1024² | ~0,5–2 с (+1–3 с загрузка) | оценка |
| RealESRGAN ×4, 1024 → 4096 | ~2–6 с | оценка (spandrel, тайлинг) |
| SeedVR2 3B int8, ×2 до 2048 | ~30–90 с | оценка. Не проверено |
| GLSL normal + roughness, 1024–2048 | < 1 с | шейдер ANGLE/D3D11 |
| TRELLIS.2 int8, проп с текстурой | ~2–5 мин, **влезание в 8 ГБ не подтверждено** | слабые источники (trellis2.app): ~120–150 с на 8 ГБ в low-VRAM режиме |
| Холодная загрузка с F: при 37 МБ/с | Z-Image ~4,7 мин, HiDream ~3,6 мин | расчёт по замеру из COMFYUI.md |

Качество. Рейтинг open-weights на Artificial Analysis на 19.10.2026: HiDream-O1-Image — 974,
Z-Image Turbo — 934, FLUX.2 klein 4B — 838, FLUX.1 schnell — 740. SD 1.5 в рейтинг уже не входит.
Сильные стороны Z-Image: чистый «продуктовый» рендер предметов, фотореализм, следование промпту.
Минусы Z-Image: при CFG 1 negative prompt не работает, разнообразие между сидами невысокое (по
отзывам, не проверено). HiDream-O1: следование промпту и детализация в 2K; при CFG 5 negative
работает.

## 5. LoRA (две, для Z-Image-Turbo)

| LoRA | Триггер | Вес по умолчанию | Для чего |
| --- | --- | --- | --- |
| tarn59/pixel_art_style_lora_z_image_turbo (12,9 тыс. скачиваний, Apache-2.0) | `Pixel art style.` | 0,9 (не проверено) | пиксель-арт иконки и кадры; результат затем приводится к сетке (§ 8) |
| artificialguybr/ICONS-REDMOND-ZIMAGETURBO (Apache-2.0) | `ICONS, ICREDM` | 0,8 (не проверено) | иконки интерфейса в стиле app-icon (меню, настройки, кнопки) |

Оговорки:
- Icons Redmond обучена на **app-иконках** (скруглённые плашки, глянец). Для предметов инвентаря
  базовый Z-Image без LoRA, скорее всего, лучше, поэтому LoRA вынесена в отдельный шаблон.
- Специальная LoRA «Game UI icon» под ZIT есть на Civitai (модель 2213526, коммерческое
  использование разрешено). Ещё там есть «Stylized Textures (Game Asset) [ZIT]» (503877) — для
  бесшовных текстур. Но скачивание с Civitai отвечает `401`: нужен личный API-ключ Civitai
  пользователя. Поэтому в план они не включены. Если пользователь даст ключ, это лучшие кандидаты
  на замену 6b и третья LoRA для PBR.
- LoRA поверх NVFP4: в ComfyUI 0.41 патчи LoRA применяются к весам в момент приведения типа
  (`comfy/model_patcher.py`, ветки `QuantizedTensor`), поэтому всё должно работать. Но качество
  на GPU **не проверено**. Если LoRA «не берёт» или картинка портится, LoRA-шаблоны переводятся
  на INT8 (P5).

## 6. Лицензии

| Компонент | Лицензия | Коммерческая игра |
| --- | --- | --- |
| Z-Image-Turbo (Tongyi-MAI, репак Comfy-Org) | Apache-2.0 | да |
| HiDream-O1-Image | MIT | да |
| BiRefNet | MIT | да |
| Real-ESRGAN (обе модели) | BSD-3-Clause | да |
| SeedVR2 | Apache-2.0 | да |
| LoRA tarn59 Pixel Art, Icons Redmond ZIT | Apache-2.0 | да |
| TRELLIS.2 (веса и интеграция) | MIT; блог Comfy: «outputs are free to use, including commercially» | да |
| DINOv3 ViT-L (энкодер для TRELLIS.2) | DINOv3 License от Meta — **не проверено**, прочитать перед коммерческим релизом | уточнить |

## 7. Image-to-3D: TRELLIS.2 — условно

TRELLIS.2 и Pixal3D в 0.41 работают на нодах ядра (`nodes_trellis2.py`, шаблон
`3d_pixal3d_trellis2_image_to_model`). Кастомные ноды, nvdiffrast и даунгрейд torch не нужны.
На выходе — меш с PBR: base color, roughness, metallic, плюс запечённые normal и AO и UV-развёртка.
INT8-DiT весит 5,25 ГБ. Ни официальных цифр VRAM, ни замеров на 8 ГБ не нашлось. Сторонние
гайды заявляют 8 ГБ при пониженном разрешении, но это источники низкого доверия.
Поэтому P4 качается последним, и на GPU-этапе нужен тест:
- разрешение структуры 512 и 1024;
- `Trellis2UpsampleStage` ниже 1536;
- при OOM — попробовать `--disable-dynamic-vram`; если не помогло — удалить 7a–7d.

Ещё одна оговорка. Встроенный шаблон подключает к `Trellis2Conditioning` файл
`dino_v3_L_naf_fp32.safetensors` из репозитория Comfy-Org/Pixal3D (1,215 ГБ), а README
Comfy-Org/TRELLIS.2 называет `dino_v3_vit_l.safetensors`. Взят файл из README. Если нода его
отвергнет, нужно докачать https://huggingface.co/Comfy-Org/Pixal3D/resolve/main/clip_vision/dino_v3_L_naf_fp32.safetensors.

Отклонено:
- **Hunyuan3D 2.x**: только форма без текстуры; лицензия Tencent Community, не действует в ЕС,
  Великобритании и Южной Корее.
- **TripoSplat**: маленький (≈4 ГБ, MIT), но выдаёт гауссовы сплаты, а меш получается через
  `SplatToMesh` — для игровых пропов качество неизвестно. Запасной вариант, если TRELLIS.2 не влезет.

## 8. Шаблоны: что добавить и с какими параметрами

### 8.1 Правила, которые следуют из кода приложения

- `default_params` (`comfy_mcp.rs:255-278`) подставляет в `{{STEPS}}`/`{{CFG}}` жёсткие 20 и 6,5
  и **не читает** meta-файл. Turbo-модели при таких значениях пережгут картинку. Поэтому новые
  шаблоны используют **свои** плейсхолдеры `"{{SAMPLER_STEPS}}"` и `"{{SAMPLER_CFG}}"` с дефолтами
  в meta (для «чужих» плейсхолдеров meta читается). Через `comfy_generate` их можно переопределить.
- Meta-файл называется `<stem>.meta.json`, где stem включает `_api`: например,
  `ui_icon_api.meta.json` (`comfy_mcp.rs:198-199`). README в `resources/comfy` пишет
  «`<имя>.meta.json`», и это вводит в заблуждение.
- Инструменты передают только `POSITIVE_PROMPT`, `NEGATIVE_PROMPT` (если задан), `SEED` и размеры
  `WIDTH`/`HEIGHT`/`FRAMES`. Внутренний размер рендера задаётся плейсхолдером `{{RENDER_SIZE}}`
  из meta. Затем `ImageScale(area)` приводит картинку к `{{WIDTH}}`×`{{HEIGHT}}`. Это убирает
  проблему SD 1.5 с шумом на кадрах 128 px.
- Z-Image работает при CFG 1. Negative собирается через `ConditioningZeroOut`, а `{{NEGATIVE_PROMPT}}`
  в Z-Image-шаблоны не включается: лишний `negative` в params безвреден.
- Имена моделей тоже вынесены в плейсхолдеры с дефолтом в meta (`{{DIFFUSION_MODEL}}`,
  `{{TEXT_ENCODER}}`, `{{VAE_NAME}}`, `{{LORA_NAME}}`). Так NVFP4 меняется на INT8 без правки графа.
- `generate_ui_icon`, `generate_pbr_material`, `generate_sprite_sheet` и `generate_3d_prop`
  жёстко берут шаблоны `ui_icon`, `pbr_material`, `sprite_sheet`, `prop3d` (`comfy_mcp.rs:36-41`).
  Варианты (LoRA, HQ) вызываются через `comfy_generate {template: ...}`.
- Прозрачность: `RemoveBackground → InvertMask → JoinImageWithAlpha → SaveImage` сохраняет RGBA PNG.
  `InvertMask` нужен, потому что `JoinImageWithAlpha` берёт alpha = 1 − mask. Сборщик атласа
  в приложении работает с `RgbaImage`, альфа сохраняется.

### 8.2 Таблица шаблонов

| Шаблон (`AI_Workflows\`) | Инструмент | Модель / ноды | Сэмплер | Шаги / CFG | Размер |
| --- | --- | --- | --- | --- | --- |
| `ui_icon_api.json` (заменить) | `generate_ui_icon` | Z-Image NVFP4 → BiRefNet → alpha → `ImageScale(area)` | `res_multistep` / `simple`, `ModelSamplingAuraFlow` shift 3 | 8 / 1,0 | рендер 1024², выход `size` (по умолчанию 512) |
| `ui_icon_app_api.json` | `comfy_generate` | то же + `LoraLoaderModelOnly(icons_redmond_zimage_turbo, 0,8)`, префикс промпта `ICONS, ICREDM, ` | то же | 8 / 1,0 | 1024² → `WIDTH` |
| `ui_icon_pixel_api.json` | `comfy_generate` | Z-Image + LoRA pixel (0,9), префикс `Pixel art style. `, затем пикселизация (ниже) | то же | 8 / 1,0 | 1024² → сетка `{{PIXEL_GRID}}`=64 → nearest до `WIDTH` |
| `sprite_sheet_api.json` (заменить) | `generate_sprite_sheet` | Z-Image, `EmptySD3LatentImage` batch `{{FRAMES}}` → BiRefNet → `ImageScale(area)` | то же | 8 / 1,0 | кадр рендерится в 768², выход `frame_size` |
| `sprite_sheet_pixel_api.json` | `comfy_generate` | как выше + LoRA pixel + пикселизация | то же | 8 / 1,0 | 768² → сетка 32–64 → nearest до `WIDTH` |
| `sprite_grid_api.json` (эксперимент) | `comfy_generate` | Z-Image, **одна** картинка 2048×1024 «sprite sheet, 4×2 grid, same character, 8 poses» → `SplitImageToTileList(512, 512, overlap 0)` → BiRefNet → `ImageScale` | то же | 8 / 1,0 | 8 кадров по 512 → `WIDTH`. Кадры согласованы между собой, но раскладку сетки модель держит не всегда — не проверено |
| `pbr_material_api.json` (заменить) | `generate_pbr_material` | Z-Image albedo → шов (GLSL сдвиг на ½ + маска креста → `SetLatentNoiseMask` → KSampler denoise 0,55 → `ImageCompositeMasked`) → GLSL normal + roughness | то же | 8 / 1,0 (шов: 8 шагов, denoise `{{SEAM_DENOISE}}`=0,55) | `resolution` (1024) |
| `pbr_material_hq_api.json` | `comfy_generate` | HiDream-O1 albedo 2048 → тот же шов (inpaint самой HiDream, 20 шагов, denoise 0,5 — не проверено; запасной вариант — Z-Image) → GLSL карты | `dpmpp_2m_sde_gpu` / `normal` | 40 / 5,0 | 2048² |
| `hero_art_api.json` | `comfy_generate` | HiDream-O1: `CheckpointLoaderSimple` → `ModelNoiseScale(8)` → `HiDreamO1PatchSeamSmoothing` → `SamplerCustom` + `BasicScheduler` + `EmptyHiDreamO1LatentImage` (скопировать из встроенного `image_hidream_o1.json` без ветки референсов и без `TextGenerate`-энхансера: промпт расширяет агент) | `dpmpp_2m_sde_gpu` / `normal` | 40 / 5,0 | 1536² по умолчанию, до 2048 |
| `upscale_api.json` | `comfy_generate` (+ `comfy_upload_image`) | `LoadImage({{INPUT_IMAGE}})` → `UpscaleModelLoader({{UPSCALE_MODEL}}=RealESRGAN_x4plus_anime_6B.pth)` → `ImageUpscaleWithModel` → `ImageScaleBy(lanczos, {{SCALE_FACTOR}}=0,5)` (итого ×2) | — | — | ×2 / ×4 |
| `upscale_hq_api.json` | `comfy_generate` | SeedVR2 3B int8 по встроенному `utility_seedvr2_3b_int8_upscale_image` (`SeedVR2Preprocess/Conditioning/PostProcessing`, `VAEEncodeTiled/DecodeTiled` 512/128), alpha через `JoinImageWithAlpha` | `euler` / `simple` | 1 / 1,0 | ×2 по умолчанию (×4 до 4K на 8 ГБ, скорее всего, слишком тяжело) |
| `pixelize_api.json` | `comfy_generate` | `LoadImage` → цепочка пикселизации | — | — | сетка `{{PIXEL_GRID}}` |
| `prop3d_api.json` (новый) | `generate_3d_prop` | Z-Image 1024² («single game prop, centered, full object visible, three-quarter view, plain white background») → `RemoveBackground` → `InvertMask` → `ImageCropToMask(1024,1024,1.1)` → TRELLIS.2 (shape → upsample → texture) из встроенного шаблона без ветки Pixal3D/MoGe → `RemeshMesh` → `DecimateMesh({{TARGET_FACES}}=60000)` → `UnwrapMesh` → `BakeTextureFromVoxel(2048)` → `BakeNormalMapFromMesh` → `SaveGLB("codewhale/prop3d")` | шаблон: KSampler shape 12 / 7,5 `euler`, texture 12 / 1 | как во встроенном | GLB; встроенный шаблон децимирует до 700 000 граней — для игры много |

Старые SD 1.5 шаблоны в `F:\AI_Drive\AI_Workflows` переименовать в `*_sd15_api.json`, чтобы
было с чем сравнивать. **Установка их не заменит:** `comfy_install_templates` пропускает уже
существующие `.json` (`comfy.rs:345-348`). Новые файлы на GPU-этапе кладутся в `AI_Workflows`
вручную, и в репозиторий в `resources/comfy` тоже.

### 8.3 Ключевые узлы

**Z-Image-ядро** — совпадает со встроенным шаблоном `image_z_image_turbo_int8.json`:
```
UNETLoader(unet_name="{{DIFFUSION_MODEL}}", weight_dtype="default")
  → [LoraLoaderModelOnly(lora_name="{{LORA_NAME}}", strength_model="{{LORA_STRENGTH}}")]   # только варианты с LoRA
  → ModelSamplingAuraFlow(shift=3) → KSampler(seed="{{SEED}}", steps="{{SAMPLER_STEPS}}", cfg="{{SAMPLER_CFG}}",
                                              sampler_name="res_multistep", scheduler="simple", denoise=1.0)
CLIPLoader(clip_name="{{TEXT_ENCODER}}", type="lumina2", device="default") → CLIPTextEncode(text="… {{POSITIVE_PROMPT}} …")
  → positive; ConditioningZeroOut(positive) → negative
EmptySD3LatentImage(width="{{RENDER_SIZE}}", height="{{RENDER_SIZE}}", batch_size=1 | "{{FRAMES}}")
VAELoader(vae_name="{{VAE_NAME}}") → VAEDecode
```
Meta для `ui_icon_api.meta.json`:
```json
{ "description": "Иконка предмета: Z-Image-Turbo 1024 → прозрачный фон → size",
  "params": { "DIFFUSION_MODEL": {"default": "z_image_turbo_nvfp4.safetensors"},
              "TEXT_ENCODER": {"default": "qwen_3_4b_fp8_mixed.safetensors"},
              "VAE_NAME": {"default": "ae.safetensors"},
              "RENDER_SIZE": {"default": 1024}, "SAMPLER_STEPS": {"default": 8}, "SAMPLER_CFG": {"default": 1.0} } }
```
Суффикс промпта иконки: `, game inventory item icon, single object centered, entire object visible,
clean readable silhouette, stylized hand-painted, soft studio light, plain flat light grey background, no text, no frame`.
Однотонный фон нужен, чтобы BiRefNet чисто вырезал предмет.

**Пикселизация** (только ноды ядра). Модель-«апскейлер» пиксель-арту не нужна: в Godot такие
текстуры показываются с фильтром Nearest. Задача — привести картинку к настоящей сетке и палитре:
```
image(1024) ──ImageScale(area → {{PIXEL_GRID}}²)──ImageQuantize(colors={{PALETTE}}=24, dither="none")──┐
mask = RemoveBackground(image) → MaskToImage → ImageScale(area → grid) → ImageToMask(red)
       → ThresholdMask(0.5) → InvertMask ─────────────────────────────────────────────────────────────┴→ JoinImageWithAlpha
       → ImageScale(nearest-exact → {{WIDTH}}) → SaveImage
```
`ImageQuantize` работает в RGB и альфу отбрасывает, поэтому альфа присоединяется после квантования.

**GLSL-карты для PBR.** Нода ядра `GLSLShader` (`comfy_extras/nodes_glsl.py`, ANGLE). В API-формате
входы называются `fragment_shader`, `size_mode` (`"from_input"`), `images.image0`, `floats.u_float0`…
Имена взяты из встроенного шаблона `basic_image_color_adjustment.json`. Перед сохранением шаблона
их нужно подтвердить через *Save (API Format)*. Текстуры загружаются с `CLAMP_TO_EDGE`, поэтому
заворот по краям сделан в шейдере вручную (`mod`) — так карты тоже получаются бесшовными.

Шейдер 1 — сдвиг на половину и маска креста (для починки шва):
```glsl
#version 300 es
precision highp float;
uniform sampler2D u_image0;
uniform float u_float0;          // half-width of the seam band as a fraction of the size, e.g. 0.06
in vec2 v_texCoord;
layout(location = 0) out vec4 fragColor0;   // image shifted by half with wrap
layout(location = 1) out vec4 fragColor1;   // white cross where the old edges are now
void main() {
  vec2 s = vec2(textureSize(u_image0, 0));
  ivec2 p = ivec2(mod(floor(v_texCoord * s) + s * 0.5, s));
  fragColor0 = vec4(texelFetch(u_image0, p, 0).rgb, 1.0);
  vec2 d = abs(v_texCoord - 0.5);
  float m = max(1.0 - smoothstep(u_float0 * 0.5, u_float0, d.x), 1.0 - smoothstep(u_float0 * 0.5, u_float0, d.y));
  fragColor1 = vec4(vec3(m), 1.0);
}
```
Дальше: `IMAGE1 → ImageToMask(red)`; `VAEEncode(IMAGE0) → SetLatentNoiseMask → KSampler(denoise 0,55)
→ VAEDecode → ImageCompositeMasked(destination=IMAGE0, source=decoded, mask)`. Края готового
albedo — это нетронутая середина исходника, поэтому текстура тайлится. Это `SaveImage("codewhale/pbr_albedo")`.

Шейдер 2 — normal (OpenGL, +Y, как ждёт Godot) и roughness из яркости:
```glsl
#version 300 es
precision highp float;
uniform sampler2D u_image0;
uniform float u_float0;          // normal strength, e.g. 2.0
uniform float u_float1;          // roughness contrast, e.g. 0.8
uniform float u_float2;          // green sign: 1.0 or -1.0 (flip if bumps look inverted in Godot)
in vec2 v_texCoord;
layout(location = 0) out vec4 fragColor0;   // normal map
layout(location = 1) out vec4 fragColor1;   // roughness (grey)
float h(vec2 p, vec2 s) {
  ivec2 q = ivec2(mod(p, s));
  return dot(texelFetch(u_image0, q, 0).rgb, vec3(0.2126, 0.7152, 0.0722));
}
void main() {
  vec2 s = vec2(textureSize(u_image0, 0));
  vec2 p = floor(v_texCoord * s);
  float tl = h(p + vec2(-1.0, 1.0), s), t = h(p + vec2(0.0, 1.0), s), tr = h(p + vec2(1.0, 1.0), s);
  float l  = h(p + vec2(-1.0, 0.0), s),                                r = h(p + vec2(1.0, 0.0), s);
  float bl = h(p + vec2(-1.0,-1.0), s), b = h(p + vec2(0.0,-1.0), s), br = h(p + vec2(1.0,-1.0), s);
  float dx = (tr + 2.0 * r + br) - (tl + 2.0 * l + bl);
  float dy = (tl + 2.0 * t + tr) - (bl + 2.0 * b + br);
  vec3 n = normalize(vec3(-dx * u_float0, -dy * u_float0 * u_float2, 1.0));
  fragColor0 = vec4(n * 0.5 + 0.5, 1.0);
  float rough = clamp(0.6 + (0.5 - h(p, s)) * u_float1, 0.05, 1.0);
  fragColor1 = vec4(vec3(rough), 1.0);
}
```
Выходы: `SaveImage("codewhale/pbr_normal")` и `SaveImage("codewhale/pbr_roughness")`. Префиксы
совпадают с `map_kind` (`comfy_mcp.rs:513`), инструмент сам соберёт `.tres` с `normal_enabled`.
Это эвристика «яркость = высота», как в CrazyBump/Materialize: для камня, кирпича и дерева
выглядит прилично, для текстур с запечёнными цветными пятнами — хуже. Ориентацию оси Y проверить
в Godot на тестовом выпуклом круге; при необходимости `u_float2 = -1`.

## 9. Правки кода приложения (не делались; для следующего этапа)

1. `comfy_mcp.rs:255-278` — брать `STEPS`/`CFG` из `meta.params` раньше жёстких 20/6,5.
   Тогда новые шаблоны смогут использовать стандартные `{{STEPS}}`/`{{CFG}}`.
2. `comfy.rs:20` — добавить новые шаблоны и их `*.meta.json` в `TEMPLATE_FILES`. Вместо пропуска
   существующих SD 1.5 шаблонов предложить обновление с бэкапом.
3. `comfy.rs:21` — добавить `"background_removal"` в `MODEL_FOLDERS` (для 3D также `"geometry_estimation"`,
   если вернётся Pixal3D/MoGe).
4. `generate_sprite_sheet`/`generate_ui_icon` — необязательный параметр `template`
   (варианты pixel/app/grid). `generate_3d_prop` — параметр таймаута: сейчас 600 с
   (`DEFAULT_TIMEOUT`), а холодный старт Z-Image + TRELLIS.2 с F: (~19 ГБ при 37 МБ/с ≈ 8,5 мин)
   в него не уложится.
5. В `resources/comfy/README.md` и `docs/COMFYUI.md` исправить имя meta-файла
   (`<имя>_api.meta.json`) и описать новые шаблоны. В `docs/COMFYUI.md` заменить совет
   «Marigold/Lotus + RMBG как кастомные ноды» на ноды ядра.
6. Чекпоинт HiDream лежит в `checkpoints` и появится в выпадающем списке «Чекпоинт». Для старых
   SD 1.5 шаблонов он не подходит — стоит отфильтровать или пометить.

## 10. Кастомные ноды и pip

**Обязательные: нет.** Весь план — ноды ядра 0.41.0.

**Необязательная** (только если GLSL-карт окажется мало): TextureAlchemy —
`git clone https://github.com/amtarr/ComfyUI-TextureAlchemy D:\AI\ComfyUI\ComfyUI\custom_nodes\ComfyUI-TextureAlchemy`,
закрепить коммит `0afda45713c7b33afbf9d4f757493cca2004f65f` (22.04.2026). Лицензия Apache-2.0.
Файла `requirements.txt` нет: импортируются только torch, numpy и PIL, плюс ленивый `imageio`
в одном экспорте. **pip не запускать.** Что даёт: Seamless Tiling Maker, Height to Normal
(sobel/scharr), конвертер DX↔GL, упаковщики каналов ORM.

Правила pip на GPU-этапе: если какая-то нода всё же потребует пакеты, ставить только так —
`python_embeded\python.exe -m pip install --dry-run -c constraints.txt …`, где в `constraints.txt`
закреплены `torch==2.14.0+cu130`, `torchvision`, `torchaudio` и `comfy_kitchen==0.2.37` в текущих
версиях (`pip freeze`). Если dry-run хочет менять torch — отказ.

Отклонено:
- **comfy_mtb (DeepBump)** — тянет `onnxruntime-gpu`, `rembg` и `transformers`: высокий риск конфликтов.
- **kijai/ComfyUI-Marigold** — тянет `diffusers`; к тому же Marigold v2 в ядре сидит на
  Qwen-Image-Edit-2509 int8 весом **20,5 ГБ** (Comfy-Org/marigold-v2-0) — не для 8 ГБ.
- **Lotus normal** (Kijai/lotus-comfyui, 1,7 ГБ) и **MoGe-2 normal** (0,66 ГБ) — обучены на сценах,
  а не на плоских текстурах. Совместимость Lotus-normal с `LotusD` ядра не проверена.
- **Nunchaku** — колёс под torch 2.14 нет.
- **ComfyUI-GGUF** — с NVFP4/INT8 в ядре не нужен.

## 11. Проверка на GPU-этапе (критерии готовности)

1. Скачать P1, сверить размеры. Добавить `background_removal` в yaml. Скопировать набор P1 на D:.
2. Сохранить новые шаблоны. Для каждого: `comfy_template_validate` (нет `missingNodeTypes`,
   `fillError` пустой).
3. `generate_ui_icon` 512: PNG с альфой, предмет целиком, ≤ 20 с после прогрева. Сравнить
   с `ui_icon_sd15`.
4. `generate_sprite_sheet` 8×128: атлас 4×2 без шума, у кадров прозрачный фон.
5. `generate_pbr_material` 1024: три карты и `.tres`. Тайлинг 3×3 в Godot без видимого шва.
6. A/B: NVFP4 против INT8 на 5 промптах, LoRA pixel/app на NVFP4. Если NVFP4 проигрывает,
   докачать INT8 (P5) и поменять `DIFFUSION_MODEL` в meta.
7. P2: `hero_art` 1536 — замерить время и пик VRAM (`comfy_status`).
8. P3/P4 — по тем же правилам. TRELLIS.2 оставить, только если проп собирается без OOM за ≤ 10 мин.

## Источники

- Z-Image-Turbo (репак Comfy-Org): https://huggingface.co/Comfy-Org/z_image_turbo ; карточка https://huggingface.co/Tongyi-MAI/Z-Image-Turbo
- Рейтинг open-weights: https://artificialanalysis.ai/image/leaderboard/text-to-image?open-weights=true
- Ориентиры скорости Z-Image: https://zimageturbo.org/comfyui-workflow , https://dev.to/gary_yan_86eb77d35e0070f5/z-image-turbo-quantized-complete-guide-to-running-professional-ai-image-generation-on-low-vram-gpus-45fb
- HiDream-O1-Image: https://huggingface.co/Comfy-Org/HiDream-O1-Image ; https://docs.comfy.org/tutorials/image/hidream/hidream-o1 ; https://comfyui-wiki.com/en/models/hidream/hidream-o1-image
- BiRefNet: https://huggingface.co/Comfy-Org/BiRefNet
- Real-ESRGAN: https://huggingface.co/Comfy-Org/Real-ESRGAN_repackaged ; https://github.com/xinntao/Real-ESRGAN/releases/tag/v0.2.2.4
- SeedVR2: https://huggingface.co/Comfy-Org/SeedVR2
- LoRA: https://huggingface.co/tarn59/pixel_art_style_lora_z_image_turbo ; https://huggingface.co/artificialguybr/ICONS-REDMOND-ZIMAGETURBO ; Civitai (нужен ключ): https://civitai.com/models/2213526 , https://civitai.com/models/503877
- TRELLIS.2: https://huggingface.co/Comfy-Org/TRELLIS.2 ; https://blog.comfy.org/p/trellis2-and-pixal3d-are-now-native ; https://trellis2.app/blog/trellis-2-low-vram (низкое доверие)
- Отклонённые: https://huggingface.co/Comfy-Org/marigold-v2-0 , https://huggingface.co/Comfy-Org/Ming-Image , https://huggingface.co/Comfy-Org/Krea-2 , https://huggingface.co/black-forest-labs/flux-3-image-klein-comfy , https://huggingface.co/Kijai/lotus-comfyui , https://huggingface.co/Comfy-Org/MoGe , https://github.com/melMass/comfy_mtb , https://huggingface.co/VAST-AI/TripoSplat
- Необязательная нода: https://github.com/amtarr/ComfyUI-TextureAlchemy
- Локально (только чтение):
  - `D:\AI\ComfyUI\ComfyUI\folder_paths.py:59,112`, `comfy_extras\nodes_glsl.py`, `nodes_bg_removal.py`, `comfy\model_patcher.py`;
  - встроенные шаблоны `python_embeded\Lib\site-packages\comfyui_workflow_templates_json\templates\`
    (`image_z_image_turbo_int8`, `image_hidream_o1`, `utility_birefnet_remove_background`,
    `utility_seedvr2_3b_int8_upscale_image`, `3d_pixal3d_trellis2_image_to_model`, `basic_image_color_adjustment`);
  - код приложения `codewhale-desktop/src-tauri/src/comfy_mcp.rs`, `comfy.rs`.
