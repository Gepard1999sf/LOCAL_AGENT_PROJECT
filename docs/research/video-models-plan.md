# Локальное видео на RTX 5060 Laptop 8 ГБ: Wan 2.2, LTX и эксперимент MiniMax H3

Дата: 2026-10-19. Это план подбора и будущего GPU-теста видеомоделей для ComfyUI на ноутбуке
пользователя. **В этой работе GPU не использовался, ничего не скачивалось и ничего не менялось
в `D:\AI\ComfyUI`.** Все цифры скорости ниже — либо чужие отчёты (со ссылкой), либо моя оценка
(помечено «оценка»), реальные числа даст протокол из § 7.

## 0. Что проверено на машине (без GPU)

| Что | Значение | Как проверено |
| --- | --- | --- |
| ComfyUI | 0.41.0 portable, `D:\AI\ComfyUI` | `ComfyUI/comfyui_version.py` |
| Python-пакеты | torch 2.14.0+cu130, `comfy_kitchen` 0.2.37, `comfy_aimdo` 0.5.6 (dynamic VRAM); **нет** `sageattention`, `triton`, `gguf` | список `python_embeded/Lib/site-packages` |
| Кастомные ноды | нет ни одной (`custom_nodes` пуст) | листинг папки |
| Поддержка в ядре | классы `WAN22_T2V`, `WAN21_I2V`, `LTXV`, `LTXAV` (LTX-2.x), `MiniMaxH3` в `comfy/supported_models.py`; ноды `Wan22ImageToVideoLatent`, `WanImageToVideo`, `WanFirstLastFrameToVideo`, `EmptyLTXVLatentVideo`, `LTXVImgToVideo`, `LTXVAddGuide`, `LTXVLatentUpsampler`, `MiniMaxH3ImageToVideo`, `ManualSigmas`; `CLIPLoader` знает типы `wan`, `ltxv`, `minimax` | grep по исходникам |
| Квант-форматы ядра | `float8_e4m3fn`, `nvfp4`, `mxfp8`, `int8_tensorwise` (+convrot), `convrot_w4a4`, `asym_w4a8_int8`, `w6a8_int8` (`comfy/quant_ops.py`); CUDA-бэкенд `comfy_kitchen` содержит `w4a8_int8_linear`, `scaled_mm_nvfp4` | grep |
| GGUF | **в ядре нет** — для любых `.gguf` нужен ComfyUI-GGUF (+ pip `gguf`), что сейчас запрещено | grep `gguf` по `comfy/` пуст |
| Пути моделей | `F:\AI_Drive\AI_ComfyModels\extra_model_paths.yaml`: `checkpoints, loras, vae, controlnet, upscale_models, clip, clip_vision, unet, embeddings, diffusion_models`. Ключ `clip` ядро отображает на `text_encoders` (`folder_paths.map_legacy`), поэтому текстовые энкодеры кладём в `clip\`. Ключей `latent_upscale_models`, `audio_encoders`, `model_patches` **нет**; `extra_model_paths.yaml.new` на момент работы не существовал | чтение файлов |
| Диски | `F:` = Samsung T7 Shield **USB** SSD (свободно 616 ГБ); `D:` = Lexar NM620 NVMe (396 ГБ); `C:` NVMe (59 ГБ) | `Get-PhysicalDisk` |
| Файл подкачки | `D:\pagefile.sys`, управляется системой, сейчас выделено 35,6 ГБ, пик использования 7 ГБ | `Win32_PageFileUsage` |
| Запуск ComfyUI приложением | `--lowvram --reserve-vram 1.0 --preview-method taesd` + `comfy.extraArgs` из настроек (`comfy.rs:246-252`). `--lowvram` при dynamic VRAM ничего не делает (`cli_args.py:171`) | чтение кода |
| MCP `comfy` | таймаут рендера по умолчанию 600 с, максимум 7200 (`comfy_mcp.rs:34`, `:760`); выводы собираются и из ключей `video`/`audio` (`comfy_mcp.rs:43`) | чтение кода |

Все URL из таблиц ниже проверены HEAD-запросом (ответ 302 на CDN, то есть файл есть и
скачивается без логина), размеры — точные байты из Hugging Face API
(`/api/models/<repo>/tree/main`). Исключение — `Lightricks/LTX-2.5`: **401, репозиторий закрыт**
(нужен вход и принятие лицензии; это пользователь делает сам).

## 1. Кратко: решение

**Фаза 1 (скачать первой, 62,30 ГБ ≈ 58,0 ГиБ, лимит 70 ГБ соблюдён):**

1. **LTX-Video 2B 0.9.8 distilled** + T5-XXL fp8 — самый быстрый вариант, «превью за минуту».
2. **Wan 2.2 TI2V-5B** (fp8 scaled от Kijai) + VAE Wan 2.2 + Turbo-LoRA на 4 шага + общий
   umT5-XXL fp8 — модель, рассчитанная на потребительские GPU, T2V и I2V в одном файле.
3. **MiniMax H3 — эксперимент:** самая маленькая официальная сборка `pruned_w6a8` (16 ГБ) +
   текстовый энкодер Qwen3-VL-32B `nvfp4_awq` + VAE видео `int8_convrot` + VAE звука `fp32` +
   Turbo-LoRA на 4 шага. Всего 37,04 ГБ — больше половины бюджета.

Всё грузится стандартными нодами ядра, **ни одной кастомной ноды и ни одного pip-пакета не
нужно**.

**Фаза 2 (после эксперимента с H3; удалить H3 → освобождается 37,04 ГБ):**
**Wan 2.2 14B I2V MoE-distill lightx2v в NVFP4** (две экспертные модели, 20,10 ГБ, + VAE Wan 2.1
0,25 ГБ). По совокупности отчётов это, скорее всего, лучшая *практичная* модель для I2V на
8 ГБ Blackwell: 4 шага, без CFG, автор сборки пишет о выборке < 60 с на RTX 5060. Опционально
— Wan 14B T2V NVFP4 (+24,0 ГБ) или LTX-2.5 22B (≈ 36 ГБ, закрытый репозиторий).

**Почему 14B не в фазе 1.** Арифметика: H3 (37,04) + umT5 (6,74) + Wan 14B I2V NVFP4 (20,35) +
LTXV 2B (≥ 9,35) = **≥ 73,5 ГБ** > 70. Выбор «H3 + Wan 5B + LTXV» укладывается в 62,3 ГБ и даёт
чистую матрицу сравнения: все три модели умеют и T2V, и I2V, поэтому все четыре тестовых
промпта (§ 7) прогоняются на каждой.

**Чего ждать (оценка для «тёплого» запуска, детали в § 6):** LTXV 2B — 0,5–1,5 мин на клип
768×512×121; Wan 5B Turbo — 2–5 мин на 1280×704×121; Wan 14B NVFP4 (фаза 2) — 2–4 мин на
832×480×81; H3 — 5–30 мин на клип, с реальным риском упереться в ОЗУ/подкачку.

## 2. Таблица файлов фазы 1

Базовый путь: `F:\AI_Drive\AI_ComfyModels`. Размер — в ГБ (10⁹ байт), в скобках — байты для
проверки после скачивания.

| # | Модель | Файл и URL | Размер, ГБ | Папка | Чем грузится |
| --- | --- | --- | --- | --- | --- |
| 1 | LTXV 2B | [`ltxv-2b-0.9.8-distilled.safetensors`](https://huggingface.co/Lightricks/LTX-Video/resolve/main/ltxv-2b-0.9.8-distilled.safetensors) | 6,34 (6 340 744 492) | `checkpoints\` | `CheckpointLoaderSimple` (модель + VAE в одном файле) |
| 2 | LTXV 2B | [`t5xxl_fp8_e4m3fn_scaled.safetensors`](https://huggingface.co/comfyanonymous/flux_text_encoders/resolve/main/t5xxl_fp8_e4m3fn_scaled.safetensors) | 5,16 (5 157 348 688) | `clip\` | `CLIPLoader`, type `ltxv` |
| 3 | Wan 5B | [`Wan2_2-TI2V-5B_fp8_e4m3fn_scaled_KJ.safetensors`](https://huggingface.co/Kijai/WanVideo_comfy_fp8_scaled/resolve/main/TI2V/Wan2_2-TI2V-5B_fp8_e4m3fn_scaled_KJ.safetensors) | 5,28 (5 277 255 650) | `diffusion_models\` | `UNETLoader` |
| 4 | Wan 5B | [`wan2.2_vae.safetensors`](https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/vae/wan2.2_vae.safetensors) | 1,41 (1 409 400 960) | `vae\` | `VAELoader` |
| 5 | Wan 5B | [`Wan22_TI2V_5B_Turbo_lora_rank_64_fp16.safetensors`](https://huggingface.co/Kijai/WanVideo_comfy/resolve/main/LoRAs/Wan22-Turbo/Wan22_TI2V_5B_Turbo_lora_rank_64_fp16.safetensors) | 0,33 (332 348 584) | `loras\` | `LoraLoaderModelOnly` |
| 6 | Wan (общий) | [`umt5_xxl_fp8_e4m3fn_scaled.safetensors`](https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors) | 6,74 (6 735 906 897) | `clip\` | `CLIPLoader`, type `wan` |
| 7 | H3 | [`minimax_h3_fl2va_pruned_w6a8.safetensors`](https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/main/diffusion_models/minimax_h3_fl2va_pruned_w6a8.safetensors) | 15,98 (15 983 746 636) | `diffusion_models\` | `UNETLoader` (нужен ComfyUI ≥ 0.38, у нас 0.41) |
| 8 | H3 | [`qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors`](https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/main/text_encoders/qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors) | 15,69 (15 687 142 551) | `clip\` | `CLIPLoader`, type `minimax` |
| 9 | H3 | [`minimax_h3_video_vae_int8_convrot.safetensors`](https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/main/vae/minimax_h3_video_vae_int8_convrot.safetensors) | 2,81 (2 811 065 184) | `vae\` | `VAELoader` |
| 10 | H3 | [`minimax_h3_audio_vae_fp32.safetensors`](https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/main/vae/minimax_h3_audio_vae_fp32.safetensors) | 0,61 (605 254 808) | `vae\` | `VAELoader` → `VAEDecodeAudio` |
| 11 | H3 | [`minimax_h3_fl2v_turbo_4step_v1.0_768p_comfyui_bf16.safetensors`](https://huggingface.co/Comfy-Org/MiniMax-H3/resolve/main/loras/minimax_h3_fl2v_turbo_4step_v1.0_768p_comfyui_bf16.safetensors) | 1,96 (1 956 192 992) | `loras\` | `LoraLoaderModelOnly` |
|  | **Итого** | LTXV 11,50 + Wan 5B 13,75 (с umT5) + H3 37,04 | **62,30** |  |  |

Резерв до лимита — 7,7 ГБ. Лучшего применения ему, чем держать место под перестановки фазы 2,
нет: ни Wan 14B (≥ 19 ГБ), ни LTX-2.5 (≈ 36 ГБ) туда не влезают.

## 3. Разбор кандидатов

### 3.1 Wan 2.2 TI2V-5B (T2V + I2V, 720p, 24 fps)

- **Файлы.** Официальный [`wan2.2_ti2v_5B_fp16.safetensors`](https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/resolve/main/split_files/diffusion_models/wan2.2_ti2v_5B_fp16.safetensors)
  — 10,00 ГБ; выбран fp8 scaled от Kijai — 5,28 ГБ. В README Kijai сказано, что его fp8 scaled
  модели работают «with ComfyUI native WanVideo nodes»
  ([README](https://huggingface.co/Kijai/WanVideo_comfy_fp8_scaled)), а автор теста на
  RTX 4060 Laptop 8 ГБ / 32 ГБ ОЗУ взял именно этот файл, потому что fp16 «слишком велик для
  8 ГБ» ([lilting.ch](https://lilting.ch/en/articles/wan22-comfyui-rtx4060-i2v)). 5,28 ГБ
  целиком помещаются в VRAM, и на Blackwell есть аппаратный FP8. GGUF Q4_K_S/Q8_0 (3,12 / 5,40 ГБ,
  [QuantStack](https://huggingface.co/QuantStack/Wan2.2-TI2V-5B-GGUF)) требуют ComfyUI-GGUF — не
  нужны.
- **VAE:** `wan2.2_vae` (1,41 ГБ, сжатие 16×16×4) — **только** для 5B; 14B использует
  `wan_2.1_vae`. **Текстовый энкодер:** `umt5_xxl_fp8_e4m3fn_scaled` (общий с 14B).
- **Ускорение.** Turbo-LoRA rank 64 (0,33 ГБ) извлечена Kijai из
  [quanhaol/Wan2.2-TI2V-5B-Turbo](https://huggingface.co/quanhaol/Wan2.2-TI2V-5B-Turbo): «121
  кадр, 24 FPS, 1280×704 за 4 шага, без CFG». Альтернатива — FastWan
  [`Wan2_2_5B_FastWanFullAttn_lora_rank_128_bf16`](https://huggingface.co/Kijai/WanVideo_comfy/resolve/main/FastWan/Wan2_2_5B_FastWanFullAttn_lora_rank_128_bf16.safetensors)
  (0,66 ГБ, 3 шага) — в набор не входит, но влезает в резерв, если Turbo окажется плохой.
  Совместимость обеих LoRA с нативным `LoraLoaderModelOnly` — **unverified** (форматы Kijai
  обычно работают).
- **Настройки.** Официальный шаблон `video_wan2_2_5B_ti2v`: `Wan22ImageToVideoLatent`
  1280×704, 121 кадр, `ModelSamplingSD3` shift 8, `KSampler` 20 шагов, CFG 5, `uni_pc`/`simple`,
  `CreateVideo` 24 fps (проверено по JSON шаблона в
  [Comfy-Org/workflow_templates](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_wan2_2_5B_ti2v.json)).
  С Turbo-LoRA: 4 шага, CFG 1, `euler`/`simple` (если мыло — `lcm`), shift 5 и 8 сравнить
  (**unverified**). Ниже ~960×544 не опускаться: на 480×480 автор теста на 4060 получил
  «искажённую форму и почти нет движения», хотя по времени это 95 с на 30 шагов (1,93 с/it,
  модель целиком в VRAM 4,86 ГБ), а на 768×480 VAE decode вылез за VRAM
  ([lilting.ch](https://lilting.ch/en/articles/wan22-comfyui-rtx4060-i2v)).
- **VRAM/ОЗУ.** Модель 5,3 ГБ + активации; umT5 6,7 ГБ работает до выборки и выгружается. Пик
  VRAM — VAE decode 720p×121 кадр: сразу ставить `VAEDecodeTiled` (tile 512, temporal 32).
  ОЗУ: ~14 ГБ с энкодером, проблем на 32 ГБ нет.
- **Скорость.** Ориентир Alibaba — «5 с 720p меньше чем за 9 минут на RTX 4090»
  ([willitrunai](https://willitrunai.com/video-models/wan-video-2-2-ti2v-5b),
  [HF forum](https://discuss.huggingface.co/t/nvidia-geforce-rtx-5060-ti-and-wan2-2-model/170645)).
  На этом ноутбуке (оценка): 20 шагов с CFG — 6–20 мин, Turbo 4 шага — 2–5 мин с decode.
  Гайд RunPod для 8-гиговых карт советует «TI2V-5B only, 480p»
  ([runpod.io](https://runpod.io/articles/guides/comfyui-wan-2-2), по пересказу поиска —
  unverified), тест покажет, держит ли 5060 нативные 720p.

### 3.2 Wan 2.2 14B (эксперты high-noise + low-noise)

Две модели по 14B: high-noise делает первые шаги, low-noise — последние; в ComfyUI это два
`UNETLoader` и два `KSamplerAdvanced` (шаги 0→2 и 2→4). Варианты:

| Вариант | Файлы | Размер | Нужно | Замечание |
| --- | --- | --- | --- | --- |
| Официальный fp8 scaled + LoRA lightx2v | `wan2.2_i2v_{high,low}_noise_14B_fp8_scaled` + `wan2.2_i2v_lightx2v_4steps_lora_v1_{high,low}_noise` ([repo](https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged/tree/main/split_files)) | 2×14,29 + 2×1,23 = 31,04 | ядро | 28,6 ГБ весов на 32 ГБ ОЗУ — эксперты не помещаются в ОЗУ одновременно, при каждом переключении перечитывание с USB-диска |
| **NVFP4 distill (рекомендация фазы 2)** | [`…-high-nvfp4`](https://huggingface.co/InsecureErasure/Wan2.2-I2V-A14B-Moe-Distill-Lightx2v-NVFP4/resolve/main/Wan2.2-I2V-A14B-Moe-Distill-Lightx2v-high-nvfp4.safetensors) 9,91 + [`…-low-nvfp4`](https://huggingface.co/InsecureErasure/Wan2.2-I2V-A14B-Moe-Distill-Lightx2v-NVFP4/resolve/main/Wan2.2-I2V-A14B-Moe-Distill-Lightx2v-low-nvfp4.safetensors) 10,19 | 20,10 | ядро + Blackwell + comfy-kitchen cu130 (всё есть) | дистилляция lightx2v уже вшита: 4 шага, CFG 1, shift 5, `euler`/`simple`, 81 кадр. Автор: «выборка < 60 с на RTX 5060, весь клип < 2 мин» ([model card](https://huggingface.co/InsecureErasure/Wan2.2-I2V-A14B-Moe-Distill-Lightx2v-NVFP4)). Только I2V |
| NVFP4 без дистилляции + LoRA | [rockerBOO](https://huggingface.co/rockerBOO/wan2.2-I2V-A14B-nvfp4) 2×8,86 + LoRA lightx2v rank64 [I2V](https://huggingface.co/lightx2v/Wan2.2-Distill-Loras) 0,63+0,74 | 19,08 | ядро | ядро умеет LoRA на NVFP4 через переквантование со стохастическим округлением (`comfy/float.py`), но качество/скорость не проверены — вшитая дистилляция надёжнее |
| T2V NVFP4 | [Xeno443](https://huggingface.co/Xeno443/Wan2.2_nvfp4) `wan2.2_t2v_{high,low}_noise_14B_fp16_nvfp4` 2×11,39 + LoRA lightx2v [T2V rank64](https://huggingface.co/lightx2v/Wan2.2-Distill-Loras) 2×0,61 | 24,02 | ядро | сборка сообщества, **unverified** |
| GGUF Q3_K_M | [QuantStack I2V](https://huggingface.co/QuantStack/Wan2.2-I2V-A14B-GGUF) `HighNoise/…-Q3_K_M.gguf` + `LowNoise/…-Q3_K_M.gguf` 2×7,18 (T2V — [тот же формат](https://huggingface.co/QuantStack/Wan2.2-T2V-A14B-GGUF), 2×7,17) + LoRA lightx2v | 14,35 + 1,2–2,5 | **ComfyUI-GGUF + pip `gguf`** (сейчас запрещено) | на Blackwell GGUF деквантуется «на лету» — медленнее NVFP4 |
| GGUF Q4_K_S | те же репозитории, `…-Q4_K_S.gguf` 2×8,75 | 17,50 + LoRA | ComfyUI-GGUF | качество выше Q3; отчёт: 8 ГБ, 480×480×81, 6 шагов с lightx2v — 23 с/it, ~181 с на клип ([runflow](https://www.runflow.io/blog/comfyui-wan-2-2-image-to-video)) |

- **Хватит ли 32 ГБ ОЗУ?** Для квантованных сборок — да: обе экспертные модели NVFP4 (20 ГБ) или
  GGUF Q4_K_S (17,5 ГБ) держатся в ОЗУ, umT5 (6,7 ГБ) выгружается после кодирования промпта.
  Подтверждение: 14B «Rapid AllInOne» (22 ГБ) на RTX 4060 Laptop / 32 ГБ отработал за 111 с
  (480×480, 33 кадра, 4 шага; 4,57 ГБ в VRAM, 11,07 ГБ выгружено в ОЗУ) и был лучше 5B и по
  качеству, и по времени ([lilting.ch](https://lilting.ch/en/articles/wan22-comfyui-rtx4060-i2v)).
  Для официального fp8 (28,6 ГБ) — впритык с перечитыванием с диска; не рекомендую.
- **Настройки** (шаблон `video_wan2_2_14B_i2v`): `WanImageToVideo` 640×640 (или 832×480), 81
  кадр, 16 fps, 4 шага (2 high + 2 low), CFG 1, `euler`/`simple`, shift 5. Петля: core-нода
  `WanFirstLastFrameToVideo` с одной и той же картинкой в начале и конце (с distill-моделью —
  **unverified**).
- **Скорость (оценка):** 2–4 мин на 832×480×81 с учётом decode; мобильная 5060 медленнее
  настольной, у автора сборки, вероятно, настольная.

### 3.3 LTX: LTX-Video 0.9.8 и LTX-2.x

- **Выбор фазы 1 — LTXV 2B 0.9.8 distilled bf16** (6,34 ГБ; VAE внутри чекпойнта, как в
  примерах ComfyUI для LTXV — подтвердить на тесте выходом `VAE` у `CheckpointLoaderSimple`) +
  `t5xxl_fp8_e4m3fn_scaled` (5,16 ГБ). Есть fp8-вариант
  [`ltxv-2b-0.9.8-distilled-fp8`](https://huggingface.co/Lightricks/LTX-Video/resolve/main/ltxv-2b-0.9.8-distilled-fp8.safetensors)
  (4,46 ГБ), но в обсуждениях репозитория жалуются на видимый водяной знак в его выводе
  ([HF discussions](https://huggingface.co/Lightricks/LTX-Video/discussions)), а экономия 1,9 ГБ
  нам не нужна.
- **Настройки.** Официальный конфиг `ltxv-2b-0.9.8-distilled.yaml` двухпроходный: первый проход
  на 2/3 разрешения с timesteps `[1.0, 0.9937, 0.9875, 0.9812, 0.975, 0.9094, 0.725]`, CFG 1, STG 0,
  затем латентный апскейлер ×2 и второй проход `[0.9094, 0.725, 0.4219]`
  ([config](https://raw.githubusercontent.com/Lightricks/LTX-Video/main/configs/ltxv-2b-0.9.8-distilled.yaml)).
  Апскейлер [`ltxv-spatial-upscaler-0.9.8`](https://huggingface.co/Lightricks/LTX-Video/resolve/main/ltxv-spatial-upscaler-0.9.8.safetensors)
  (0,51 ГБ) грузится нодой ядра `LTXVLatentUpsampler` из `latent_upscale_models`, а этого ключа
  в нашем `extra_model_paths.yaml` нет. Поэтому в фазе 1 — **один проход**: `ManualSigmas`
  `1.0, 0.9937, 0.9875, 0.9812, 0.975, 0.9094, 0.725, 0.4219, 0.0`, `euler`, CFG 1, 768×512
  (кратно 32), 121 кадр (8k+1), `LTXVConditioning` 24 fps; I2V — `LTXVImgToVideo`, петля —
  `LTXVAddGuide` с той же картинкой на последнем кадре (**unverified** для 2B distilled).
  Двухпроходный режим — после того как в yaml появится `latent_upscale_models`.
- **Скорость.** Исходный LTXV 2B: «121 кадр 768×512 за 4 с при 20 шагах» на RTX 4090
  ([blog.comfy.org](https://blog.comfy.org/ltxv-day-1-comfyui/)). Замеров 0.9.8 2B на 8 ГБ
  найти не удалось. Оценка для этого ноутбука: 0,5–1,5 мин на клип, VRAM 6–7,5 ГБ, ОЗУ ~12 ГБ.
  Качество — самое низкое в наборе; роль модели — быстрые превью и простые явления (вода, огонь).
- **LTXV 13B 0.9.8 distilled fp8** ([15,69 ГБ](https://huggingface.co/Lightricks/LTX-Video/resolve/main/ltxv-13b-0.9.8-distilled-fp8.safetensors))
  — промежуточный вариант, в бюджет фазы 1 не влез.
- **LTX-2.x (2.3 — 22B; 2.5 — 22B, вышла 2026-10-11, видео + звук).** Файлы
  [Lightricks/LTX-2.5](https://huggingface.co/Lightricks/LTX-2.5) (репозиторий закрыт, 401):
  distilled NVFP4 18,72 ГБ, int8 convrot 21,50, bf16 42,02; текстовый энкодер
  `gemma4-12b-with-proj-ltx-2.5` int8 convrot 15,37 / bf16 26,26 (сообщество: NVFP4 10,60 ГБ,
  [Deadshot699](https://huggingface.co/Deadshot699/ltx-2.5-gemma4-12b-comfy-nvfp4)); VAE видео
  1,47 + звука 0,36; апскейлеры 1,00 / 0,26. Минимальный набор на ядре — **≈ 35,9 ГБ**, то есть
  больше 32 ГБ ОЗУ одними весами. GGUF ([unsloth/LTX-2.5-GGUF](https://huggingface.co/unsloth/LTX-2.5-GGUF),
  открыт): distilled Q3_K_M 11,30, UD-Q3_K_XL 12,00, Q4_K_M 13,58 — нужен ComfyUI-GGUF. Для LTX-2.3
  есть workflow «8 GB + 32 GB» ([civitai](https://civitai.com/models/2477099/ltx-23-low-vram-8gb-32g-workflow)),
  NVIDIA заявляет для NVFP4 на RTX 50 «до 3× быстрее и на 60 % меньше VRAM»
  ([NVIDIA blog](https://blogs.nvidia.com/blog/rtx-ai-garage-ces-2026-open-models-video-generation%27)),
  но независимых замеров на 8-гиговых ноутбуках нет. Итог: LTX-2.5 — кандидат фазы 2 вместо H3,
  если пользователь примет лицензию на HF.

### 3.4 MiniMax H3 — эксперимент «самая маленькая сборка с подкачкой»

**Что это.** Видео + звук совместно (T2V, I2V по первому/последнему кадру — файлы `fl2va`,
референсы — `ref2va`), 24 fps, длина на сетке 17k+5 (73 кадра ≈ 3 с, 124 ≈ 5,2 с, обучено на
124–362). Полная модель 33,1B, «pruned» — это не обрезка качества, а перестройка AdaLN
(13B параметров модуляции → 0,04B), 20,1B параметров
([lilcheaty README](https://huggingface.co/lilcheaty/MiniMax-H3-NVFP4)). Текстовый энкодер —
**Qwen3-VL-32B, обрезанный до 50 слоёв** (`comfy/text_encoders/minimax.py`), это второй гигант
в наборе.

**Сборки DiT (`fl2va`) и выбор:**

| Сборка | Размер | Где | Ядро? | Замечание |
| --- | --- | --- | --- | --- |
| **`pruned_w6a8`** | **15,98** | [Comfy-Org](https://huggingface.co/Comfy-Org/MiniMax-H3) | да, ≥ 0.38 | **выбрана**: самая маленькая официальная, int6-веса / int8-активации, ядро CUDA есть в `comfy_kitchen` |
| `pruned_int8_convrot` | 20,97 | Comfy-Org | да | по умолчанию в официальных шаблонах |
| `pruned_fp8_scaled` | 20,96 | Comfy-Org | да | «только если нельзя int8_convrot» |
| `pruned_bf16` / `bf16` | 40,23 / 66,28 | Comfy-Org | да | не для этой машины |
| `pruned_nvfp4` | 12,53 | [lilcheaty](https://huggingface.co/lilcheaty/MiniMax-H3-NVFP4/resolve/main/minimax_h3_fl2va_pruned_nvfp4.safetensors) (зеркало у [Abiray](https://huggingface.co/Abiray/Minimax-H3-nvfp4-INT4-INT8-Convrot)) | да (`comfy_quant` nvfp4) | **запасной шаг вниз**: −3,45 ГБ, на RTX PRO 6000 −12 % времени шага; но автор сам пишет, что `fl2va` end-to-end не прогонялся и 4 бита заметно портят движение |
| `pruned_w4a8_mixed` | 12,54 | [AX1Y2JP](https://huggingface.co/AX1Y2JP/MiniMax-H3-W4A8-ConvRot) | вероятно (`asym_w4a8_int8`), **unverified** | сообщество |
| GGUF Q3_K_M / Q4_K_M | 8,90 / 11,56 | [Abiray](https://huggingface.co/Abiray/MiniMax-H3-Pruned-GGUF) | нет, ComfyUI-GGUF | нельзя сейчас |

**Текстовый энкодер:** `nvfp4_awq` 15,69 ГБ — самый маленький из поддерживаемых ядром
(int8_convrot 27,14; bf16 51,51). Comfy-Org: «этот nvfp4 энкодер не требует Blackwell». GGUF-
варианты от 6,91 ГБ ([nif0](https://huggingface.co/nif0/Qwen3-VL-32B-Instruct-MiniMax-H3-GGUF))
требуют кастомной ноды с поддержкой Qwen3-VL — **unverified**.

**VAE:** видео `int8_convrot` 2,81 ГБ («декодирует ~2× быстрее» по
[docs.comfy.org](https://docs.comfy.org/tutorials/video/minimax/minimax-h3)), звук — только
`fp32` 0,61 ГБ (fp16 даёт рассинхрон звука, по сводке
[wildminder](https://github.com/wildminder/awesome-minimax-H3/blob/main/guides/minimax-h3-performance.md)).
Аудио-VAE для генерации не обязателен: латент AV создаётся без него, он нужен только
`VAEDecodeAudio`.

**LoRA ускорения:** `minimax_h3_fl2v_turbo_4step_v1.0_768p_comfyui_bf16` (1,96 ГБ) — та же,
что сейчас стоит в онлайн-студии lightx2v ([lightx2v/Minimax-h3-Turbo](https://huggingface.co/lightx2v/Minimax-h3-Turbo));
там же есть v1.1/v1.2 и 8-шаговые версии (официальные шаблоны ComfyUI берут 8-шаговую, docs
советуют «turbo on: 8 шагов для T2V/I2V»). 4 шага — ради времени; при плохом качестве — 8 шагов
той же LoRA (докачать 1,96 ГБ).

**Итого H3: 37,04 ГБ.** «Block swap» в ядре ComfyUI отдельно не настраивается: dynamic VRAM
(`comfy_aimdo`) сам стримит веса из ОЗУ в VRAM по слоям — это и есть block swap.

**Настройки для теста** (официальные шаблоны
[`video_minimax_h3_t2v`](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_minimax_h3_t2v.json) /
[`video_minimax_h3_i2v`](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_minimax_h3_i2v.json),
только замена файла DiT и LoRA): `UNETLoader` w6a8, `CLIPLoader` type `minimax`,
`LoraLoaderModelOnly` turbo 1.0, `MiniMaxH3ImageToVideo` (first_frame / last_frame —
опционально; одна картинка в обоих = петля), `KSamplerSelect` `res_multistep`,
`BasicScheduler` `simple` 4 шага, shift видео 12 / звука 3, `VAEDecode` (лучше
`VAEDecodeTiled`, temporal 16–32) + `VAEDecodeAudio`, `CreateVideo` 24 fps. Начать с 608×352
(0,2 МП) × 73 кадра, затем 864×480 (0,4 МП, стартовое разрешение шаблонов) × 124. Промпт H3 ждёт
структурированного описания (`integrated_multimodal_description`, `overall_soundscape`, камера
«тип + амплитуда + скорость», 350–500 слов), иначе качество проседает и речь превращается в
бессмыслицу ([guide](https://huggingface.co/MiniMaxAI/MiniMax-H3/blob/main/docs/VIDEO_PROMPT_WRITING_GUIDE_base_en.md)).

**Бюджет памяти на 32 ГБ ОЗУ (оценка):**

| Этап | В ОЗУ | Комментарий |
| --- | --- | --- |
| Кодирование промпта | энкодер 15,7 + Windows/приложения ~5–6 | энкодер стримится в 8 ГБ VRAM, один проход на промпт |
| Выборка | DiT 16,0 + LoRA 2,0 + рабочие буферы ~2–3 + Windows ~5–6 ≈ 25–27 | **энкодер должен быть выгружен**; если кэш его держит — 41+ ГБ, то есть подкачка |
| Декод | VAE 2,8 + 0,6 + кадры | VRAM-пик: декод 124 кадров |

Пиковая потребность в коммите — 45–55 ГБ, при 32 ГБ физической памяти подкачка неизбежна
хотя бы на переходе «энкодер → DiT».

**Что нужно, чтобы попробовать:**
1. **Подкачка на `D:` (NVMe).** Сейчас она управляется системой (35,6 ГБ). Для эксперимента —
   задать вручную исходный 49 152 МБ / максимальный 65 536 МБ на `D:` (не на `F:` — это USB).
   Это системная настройка, пользователь меняет её сам: «Система → О системе → Дополнительные
   параметры → Быстродействие → Дополнительно → Виртуальная память», затем перезагрузка.
2. **Освободить ОЗУ:** закрыть Godot, браузер, WSL/Docker (в `docs/COMFYUI.md` они держали
   ~13 ГБ), Ollama выгрузит эстафета VRAM.
3. **Флаги** — через настройку приложения `comfy.extraArgs`, по одному за прогон:
   - базовый: флаги приложения как есть; в логе найти `Model storage policy: fast_disk=...`
     (ядро само решает, стримить ли веса с диска; для USB `F:` ожидается `False`) и
     `Native ops: ...` (должны быть `nvfp4`, `w6a8_int8`);
   - `--cache-none` — не держать энкодер и выходы нод в ОЗУ между этапами (главный кандидат);
   - `--disable-mmap` — веса копируются в приватную память: при нехватке ОЗУ они уходят в
     подкачку на NVMe `D:`, а не перечитываются с USB `F:` (с mmap Windows сбрасывает страницы
     файла и читает их заново с диска); в сводке wildminder этот флаг лечит падение Windows на
     safetensors > 20 ГБ (у нас файлы ≤ 16 ГБ);
   - `--fast-disk` — принудительно стримить веса с диска вместо ОЗУ; разумно, только если DiT
     лежит на NVMe (вариант: скопировать 16 ГБ в `D:\AI_Models\diffusion_models` и добавить путь
     в yaml — позже и с согласия пользователя);
   - `--reserve-vram 1.5` / `--vram-headroom 0.5` — если зависает рабочий стол;
     `--disable-pinned-memory` — если система замирает (так лечили фризы в отчётах);
   - `--novram` — последний шаг, всё через ОЗУ, очень медленно.
4. **Таймаут MCP:** `comfy_generate` с `timeout_sec` 3600–7200 (по умолчанию 600 с не хватит).

**Ожидаемые сбои:**
- нехватка ОЗУ на переходе энкодер → DiT: Python падает с `MemoryError` или Windows пишет
  «недостаточно памяти»; лечится `--cache-none` и подкачкой;
- «трэшинг» подкачки: секунды на шаг растут в 5–20 раз, загрузка GPU в `nvidia-smi` < 30 %, диск
  100 %. Пример масштаба: FramePack на RTX 4060 Laptop / 32 ГБ с моделью 26 ГБ — 56 мин на 5 с
  ([lilting.ch](https://lilting.ch/en/articles/local-long-video-fastwan-framepack-rocm)).
  Критерий остановки: первый шаг дольше 10 мин;
- CUDA OOM выше ~0,4 МП или на декоде → меньше МП / кадров, `VAEDecodeTiled`; в отчётах
  «декод висит на 0 %» лечили сменой VAE на int8;
- LoRA на `w6a8` может долго переквантоваться при загрузке или не примениться → прогон без
  LoRA на 8–12 шагах или сборка `pruned_nvfp4`;
- артефакты INT8-внимания (морфинг в конце клипа) — известная особенность, по docs;
- драйвер/перегрев: у владельца RTX 4050 Laptop после серии тяжёлых прогонов был
  `VIDEO_DXGKRNL_FATAL_ERROR` при 75–85 °C (сводка wildminder) — следить за температурой.

**Отчёты других (ни одного замера H3 на 8 ГБ с временем нет):**
- RTX 4060 Laptop 8 ГБ: ref2va int8_convrot + энкодер nvfp4 + Sage, 0,4 МП, 20 шагов — «работает»,
  без времени; RTX 3050 6 ГБ + 24 ГБ ОЗУ, INT4 + Turbo 8 шагов: 0,2 МП × 10 с ≈ 5 мин,
  0,3 МП ≈ 10 мин, OOM выше 0,4 МП; RTX 3060 12 ГБ: официальные 20 шагов 1280×736 × 5 с — 1006 с,
  Turbo 4 шага + EasyCache — 420 с ([wildminder](https://github.com/wildminder/awesome-minimax-H3/blob/main/guides/minimax-h3-performance.md));
- RTX 3060 12 ГБ + 32 ГБ + быстрый NVMe: 864×480, 124 кадра, 20 шагов — меньше 9 минут; гайд
  прямо пишет «8 ГБ не считать поддерживаемым» ([kingy.ai](https://kingy.ai/ai/ai-guides/minimax-h3-comfyui-local-guide/));
- на 1152×640 × 362 кадра даже 96-гиговая карта даёт ~29 с/it; 1344×768 × 362 — OOM
  ([lilcheaty](https://huggingface.co/lilcheaty/MiniMax-H3-NVFP4)).

**Оценка для этой машины:** 608×352 × 73 кадра × 4 шага — 4–10 мин «тёпло», первый запуск
+3–6 мин (37 ГБ с USB при ~0,9 ГБ/с — минимум 45 с, плюс энкодер); 864×480 × 124 — 10–30 мин,
если не начнётся трэшинг. Вероятность, что эксперимент окажется практичным, низкая; цель —
выяснить предел, а не получить рабочий инструмент.

## 4. Общие текстовые энкодеры и VAE

- `umt5_xxl_fp8_e4m3fn_scaled` (6,74 ГБ) — **один на все Wan** (5B, 14B T2V/I2V, Wan 2.1).
- LTXV 0.9.x использует **T5 v1.1 XXL** (`t5xxl_*`) — другая модель и другой токенизатор, с umT5
  не совмещается; тот же файл подходит Flux.1 / SD3.5. LTX-2.x — Gemma (своя), H3 — Qwen3-VL-32B
  (свой). Общего энкодера между семействами нет.
- VAE не взаимозаменяемы: Wan 5B — `wan2.2_vae` (1,41), Wan 14B — `wan_2.1_vae` (0,25), LTXV — в
  чекпойнте, H3 — свои видео/аудио VAE.

**Объёмы скачивания:**

| Набор | ГБ |
| --- | --- |
| Фаза 1 (LTXV 2B + Wan 5B + H3) | 62,30 |
| Фаза 2а: Wan 14B I2V NVFP4 distill + `wan_2.1_vae` | +20,35 |
| После фазы 2а с удалением H3 | 45,61 на диске |
| Фаза 2б: Wan 14B T2V NVFP4 + LoRA T2V | +24,02 (итого 69,63 без H3) |
| Альтернатива 2в вместо 2б: LTX-2.5 distilled NVFP4 + Gemma4 int8 + VAE | +35,93 (вместе с 2а уже 81,5 ГБ — только если убрать Wan 5B и LTXV 2B или поднять лимит) |

## 5. Решения, отклонённые сознательно

- **GGUF где угодно** — нужен ComfyUI-GGUF и pip-пакет в `D:\AI\ComfyUI`; на Blackwell NVFP4 /
  FP8 / int8 из ядра ещё и быстрее.
- **Wan 5B fp16 официальный** — +4,7 ГБ без выигрыша на 8 ГБ VRAM; если fp8 даст заметную
  потерю качества, его можно докачать в резерв (10,00 ГБ).
- **H3 `pruned_int8_convrot`** (по умолчанию в шаблонах) — 21 ГБ на 32 ГБ ОЗУ хуже, чем 16 ГБ.
- **H3 `pruned_nvfp4`** как основной — меньше, но `fl2va` не проверен автором, а 4 бита по его
  же словам портят движение; оставлен шагом вниз.
- **SageAttention** — ускоряет H3 «примерно в 2×» (docs), но требует pip-установки; к тому же
  на ≤ 12 ГБ сводка wildminder пишет о замедлении 2,5–3× из-за роста выгрузки. Только после
  базовых замеров и с отдельного разрешения.

## 6. Сводка ожиданий (оценки, проверить § 7)

| Модель | Разрешение × кадры | Шаги / CFG | Время на клип, тёпло | VRAM пик | ОЗУ пик | Качество | Опора |
| --- | --- | --- | --- | --- | --- | --- | --- |
| LTXV 2B 0.9.8 distilled | 768×512 × 121 (24 fps) | 8 / 1 | 0,5–1,5 мин | 6–7,5 ГБ | ~12 ГБ | низкое, быстрые превью | 4090: 4 с на 20 шагов (blog.comfy.org); пересчёт мой |
| Wan 2.2 5B + Turbo | 1280×704 × 121 (24 fps) | 4 / 1 | 2–5 мин | ~7,5 ГБ (декод тайлами) | ~14 ГБ | среднее; плохо ниже ~960×544 | 4060L: 480×480, 30 шагов, 95 с (lilting.ch) |
| Wan 2.2 5B без LoRA | 1280×704 × 121 | 20 / 5 | 6–20 мин | ~7,5 ГБ | ~14 ГБ | эталон для сравнения Turbo | 4090: < 9 мин (Alibaba) |
| H3 w6a8 + Turbo 4 | 608×352 × 73 | 4 / — | 4–10 мин (+3–6 мин холодный) | ~7,5 ГБ (всё стримится) | 32 ГБ + подкачка 10–25 ГБ | потенциально лучшее + звук | 3050 6 ГБ: 0,2 МП × 10 с ≈ 5 мин (wildminder) |
| H3 w6a8 + Turbo 4 | 864×480 × 124 | 4 / — | 10–30 мин или трэшинг | 7,5+ ГБ | то же | то же | 3060 12 ГБ: < 9 мин на 20 шагах (kingy.ai) |
| Фаза 2: Wan 14B I2V NVFP4 | 832×480 × 81 (16 fps) | 4 (2+2) / 1 | 2–4 мин | ~7,5 ГБ | ~24 ГБ | хорошее, лучший I2V | RTX 5060: < 2 мин (model card) |

## 7. Протокол GPU-теста (выполнить позже, когда GPU свободен)

### 7.1 Подготовка
1. Скачать файлы фазы 1 (§ 8), сверить размеры в байтах с § 2.
2. Питание от сети, профиль производительности; закрыть Godot, браузер, WSL/Docker. Записать
   версию драйвера (`nvidia-smi`), свободную ОЗУ, размер подкачки.
3. Запустить ComfyUI из приложения (кнопка запуска; флаги приложения + `comfy.extraArgs`).
   Сохранять консольный лог: приложение пишет stdout в null, поэтому для теста запустить
   вручную тем же набором аргументов с перенаправлением в
   `F:\AI_Drive\AI_Workflows\video_tests\logs\comfy_<model>_<run>.log`.
4. Собрать графы из официальных шаблонов (Workflow → Templates) с заменой файлов на наши,
   сохранить как API-формат в `F:\AI_Drive\AI_Workflows\video_tests\graphs\`.
5. Подготовить входную картинку персонажа (полный рост, нейтральный фон, 704×1280 или
   768×768) — например, из SD 1.5, уже стоящей в `checkpoints`, — в
   `F:\AI_Drive\AI_Workflows\video_tests\inputs\character.png`, загрузить через
   `comfy_upload_image`.

### 7.2 Мониторинг (PowerShell 5.1, запускать перед каждым прогоном)
```powershell
$d = 'F:\AI_Drive\AI_Workflows\video_tests\metrics'; New-Item -ItemType Directory -Force $d | Out-Null
$tag = 'wan5b_turbo_water_s42'
# VRAM / загрузка / температура каждые 500 мс
$gpu = Start-Process nvidia-smi -ArgumentList '--query-gpu=timestamp,memory.used,memory.total,utilization.gpu,temperature.gpu,power.draw,clocks.sm','--format=csv','-lms','500','-f',"$d\$tag.gpu.csv" -PassThru -WindowStyle Hidden
# ОЗУ, коммит, подкачка, чтение с дисков каждую секунду
$ram = Start-Process typeperf -ArgumentList '"\Memory\Available MBytes"','"\Memory\Committed Bytes"','"\Paging File(_Total)\% Usage"','"\PhysicalDisk(_Total)\Disk Read Bytes/sec"','"\Process(python)\Private Bytes"','-si','1','-f','CSV','-o',"$d\$tag.ram.csv" -PassThru -WindowStyle Hidden
# ... прогон ...
Stop-Process -Id $gpu.Id, $ram.Id
```
Пик VRAM = максимум `memory.used`; пик ОЗУ = минимум `Available MBytes` и максимум
`Committed Bytes`; трэшинг = подкачка растёт, а `utilization.gpu` < 30 %.

### 7.3 Промпты (одни и те же для всех моделей, английский)
1. **Петля воды (T2V):** «Top-down view of a calm stylized water surface for a game, gentle
   ripples and soft caustic light patterns drifting slowly, no objects, no shoreline, even
   lighting, static camera, seamless looping motion.»
2. **Петля факела (T2V):** «Close-up of a medieval wall torch burning at night, flickering orange
   flames and small rising sparks, dark stone wall background, static camera, steady seamless
   loop, game VFX reference.»
3. **Idle персонажа (I2V, `character.png`):** «The character stands in a relaxed idle pose,
   subtle breathing, slight weight shift, occasional blink, clothes and hair sway gently,
   static camera, plain background, no camera movement.»
4. **Кинематографичный пролёт 5 с (T2V):** «Slow cinematic dolly-in through a misty fantasy
   forest toward an ancient stone gate, volumetric god rays, drifting fog, 24 fps film look,
   smooth camera motion.»

Для Wan — негатив из официального шаблона. Для H3 — те же сцены, переписанные в его
структурный формат (`integrated_multimodal_description` + `overall_soundscape`: шум воды,
треск огня, лесной фон). Петли: H3 — first = last = один кадр (кадр взять из T2V-клипа той же
модели); Wan 14B — `WanFirstLastFrameToVideo`; LTXV — `LTXVAddGuide` на последний кадр; Wan 5B
FLF в ядре не умеет — оценить шов как есть.

### 7.4 Матрица и порядок
Seed 42 и 1234 на каждую ячейку; первый прогон модели — «холодный», второй — «тёплый».

| Шаг | Модель | Настройки | Промпты |
| --- | --- | --- | --- |
| 1 | LTXV 2B distilled | 768×512 × 121, 8 сигм, CFG 1 | 1–4 |
| 2 | LTXV 2B distilled | 1216×704 × 121 (проверка предела) | 4 |
| 3 | Wan 5B + Turbo | 1280×704 × 121, 4 шага, CFG 1, shift 5 и 8 | 1–4 |
| 4 | Wan 5B без LoRA | 1280×704 × 121, 20 шагов, CFG 5 (эталон) | 3 |
| 5 | Wan 5B + Turbo | 960×544 × 121 (если шаг 3 упёрся в память) | 1–4 |
| 6 | H3, отдельная сессия, после перезагрузки | лестница § 7.6 | 1, 3 → остальные при успехе |
| 7 | Фаза 2: Wan 14B I2V NVFP4 | 832×480 × 81, 4 шага (2+2), CFG 1, shift 5 | 3 (+ 1, 2 через первый кадр из T2V) |

### 7.5 Что записывать (таблица результатов)
`модель | сборка | промпт | seed | WxH×кадры | шаги/CFG | флаги | холодный, с | тёплый, с | с/it |
пик VRAM, МБ | мин. свободная ОЗУ, МБ | пик коммита, ГБ | пик подкачки, % | чтение с диска, ГБ |
сбой (OOM / MemoryError / таймаут / зависание / TDR) | качество 1–5: движение, стабильность во
времени, шов петли, следование промпту, артефакты`. Время — `elapsedMs` из ответа
`comfy_generate` и разница `execution_start`/`execution_success` в `/history/<id>`.
Шов петли — визуально, плюс при желании сравнение первого и последнего кадра через PyAV из
`python_embeded` (ffmpeg в PATH нет).

### 7.6 Лестница эксперимента H3 (менять одну переменную за раз)
1. 608×352 × 73, Turbo 4, флаги приложения. Записать строки `Model storage policy` и
   `Native ops`.
2. То же + `--cache-none`.
3. То же + `--disable-mmap` (сравнить пик коммита и чтение с `F:`).
4. Лучший из 1–3 на 864×480 × 124.
5. При OOM на декоде — `VAEDecodeTiled` temporal 16; при сбое LoRA — без LoRA на 8 шагах.
6. При неустранимой нехватке памяти — шаг вниз на `pruned_nvfp4` (12,53 ГБ, после удаления
   `w6a8`) или `--novram`.
Стоп-критерии: первый шаг > 10 мин, температура GPU > 87 °C, зависание рабочего стола.

### 7.7 Критерии вывода
- «Пригодно для работы»: тёплый клип ≤ 5 мин при ≥ 480p, без трэшинга, качество ≥ 3/5 на
  промптах 1–3.
- H3 «эксперимент удался»: 0,2 МП × 73 кадра завершился без падения за ≤ 30 мин.
- После теста: удалить проигравшие файлы, скачать фазу 2, оформить шаблоны приложения
  (`video_ltxv_t2v_api.json`, `video_wan5b_ti2v_api.json`, `video_wan14b_i2v_api.json`, при
  успехе — `video_h3_api.json`) с плейсхолдерами `{{POSITIVE_PROMPT}}`, `{{NEGATIVE_PROMPT}}`,
  `"{{SEED}}"`, `"{{WIDTH}}"`, `"{{HEIGHT}}"`, `"{{FRAMES}}"`, `"{{STEPS}}"`, `"{{CFG}}"`,
  `{{INPUT_IMAGE}}` и нодой `SaveVideo` (MCP уже собирает ключ `video`), плюс `.meta.json` с
  рекомендуемым `timeout_sec`.

## 8. Скачивание (когда пользователь разрешит)

Git Bash, в фоне, с докачкой; проверка завершения — цикл по размеру в байтах:
```bash
M=/f/AI_Drive/AI_ComfyModels
dl() { # $1 url, $2 target, $3 bytes
  until [ "$(stat -c %s "$2" 2>/dev/null)" = "$3" ]; do
    curl -L --fail --retry 20 --retry-all-errors -C - -o "$2.part" "$1" && mv "$2.part" "$2" || sleep 30
  done; echo "OK $2"; }
dl https://huggingface.co/Lightricks/LTX-Video/resolve/main/ltxv-2b-0.9.8-distilled.safetensors "$M/checkpoints/ltxv-2b-0.9.8-distilled.safetensors" 6340744492
dl https://huggingface.co/comfyanonymous/flux_text_encoders/resolve/main/t5xxl_fp8_e4m3fn_scaled.safetensors "$M/clip/t5xxl_fp8_e4m3fn_scaled.safetensors" 5157348688
# ... остальные строки по таблице § 2 (URL, папка, байты)
```
Порядок: LTXV → Wan 5B → H3 (самое большое — последним, чтобы первые тесты могли начаться
раньше). Запись идёт на USB-диск `F:` (~0,9 ГБ/с максимум), скорость ограничит сеть.

## 9. Источники

- MiniMax H3: [docs.comfy.org](https://docs.comfy.org/tutorials/video/minimax/minimax-h3),
  [Comfy-Org/MiniMax-H3](https://huggingface.co/Comfy-Org/MiniMax-H3),
  [lightx2v/Minimax-h3-Turbo](https://huggingface.co/lightx2v/Minimax-h3-Turbo),
  [lilcheaty/MiniMax-H3-NVFP4](https://huggingface.co/lilcheaty/MiniMax-H3-NVFP4),
  [Abiray/MiniMax-H3-Pruned-GGUF](https://huggingface.co/Abiray/MiniMax-H3-Pruned-GGUF),
  [AX1Y2JP/MiniMax-H3-W4A8-ConvRot](https://huggingface.co/AX1Y2JP/MiniMax-H3-W4A8-ConvRot),
  [nif0 TE GGUF](https://huggingface.co/nif0/Qwen3-VL-32B-Instruct-MiniMax-H3-GGUF),
  [wildminder performance](https://github.com/wildminder/awesome-minimax-H3/blob/main/guides/minimax-h3-performance.md),
  [kingy.ai guide](https://kingy.ai/ai/ai-guides/minimax-h3-comfyui-local-guide/),
  [prompt guide](https://huggingface.co/MiniMaxAI/MiniMax-H3/blob/main/docs/VIDEO_PROMPT_WRITING_GUIDE_base_en.md),
  шаблоны [t2v](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_minimax_h3_t2v.json) /
  [i2v](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_minimax_h3_i2v.json).
- Wan 2.2: [Comfy-Org/Wan_2.2_ComfyUI_Repackaged](https://huggingface.co/Comfy-Org/Wan_2.2_ComfyUI_Repackaged),
  [Kijai/WanVideo_comfy_fp8_scaled](https://huggingface.co/Kijai/WanVideo_comfy_fp8_scaled),
  [Kijai/WanVideo_comfy](https://huggingface.co/Kijai/WanVideo_comfy),
  [quanhaol/Wan2.2-TI2V-5B-Turbo](https://huggingface.co/quanhaol/Wan2.2-TI2V-5B-Turbo),
  [InsecureErasure NVFP4 distill](https://huggingface.co/InsecureErasure/Wan2.2-I2V-A14B-Moe-Distill-Lightx2v-NVFP4),
  [rockerBOO NVFP4](https://huggingface.co/rockerBOO/wan2.2-I2V-A14B-nvfp4),
  [Xeno443 NVFP4](https://huggingface.co/Xeno443/Wan2.2_nvfp4),
  [lightx2v/Wan2.2-Distill-Loras](https://huggingface.co/lightx2v/Wan2.2-Distill-Loras),
  [lightx2v/Wan2.2-Lightning](https://huggingface.co/lightx2v/Wan2.2-Lightning),
  [QuantStack I2V GGUF](https://huggingface.co/QuantStack/Wan2.2-I2V-A14B-GGUF),
  [QuantStack T2V GGUF](https://huggingface.co/QuantStack/Wan2.2-T2V-A14B-GGUF),
  [QuantStack 5B GGUF](https://huggingface.co/QuantStack/Wan2.2-TI2V-5B-GGUF),
  шаблоны [5B](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_wan2_2_5B_ti2v.json) /
  [14B I2V](https://github.com/Comfy-Org/workflow_templates/blob/main/templates/video_wan2_2_14B_i2v.json),
  отчёты [lilting.ch 4060](https://lilting.ch/en/articles/wan22-comfyui-rtx4060-i2v),
  [lilting.ch FramePack/RAM](https://lilting.ch/en/articles/local-long-video-fastwan-framepack-rocm),
  [runflow](https://www.runflow.io/blog/comfyui-wan-2-2-image-to-video),
  [HF forum 5060 Ti](https://discuss.huggingface.co/t/nvidia-geforce-rtx-5060-ti-and-wan2-2-model/170645),
  [willitrunai](https://willitrunai.com/video-models/wan-video-2-2-ti2v-5b).
- LTX: [Lightricks/LTX-Video](https://huggingface.co/Lightricks/LTX-Video),
  [config 2B 0.9.8 distilled](https://raw.githubusercontent.com/Lightricks/LTX-Video/main/configs/ltxv-2b-0.9.8-distilled.yaml),
  [HF discussions](https://huggingface.co/Lightricks/LTX-Video/discussions),
  [blog.comfy.org LTXV](https://blog.comfy.org/ltxv-day-1-comfyui/),
  [comfyanonymous/flux_text_encoders](https://huggingface.co/comfyanonymous/flux_text_encoders),
  [Lightricks/LTX-2.5](https://huggingface.co/Lightricks/LTX-2.5) (закрыт),
  [unsloth/LTX-2.5-GGUF](https://huggingface.co/unsloth/LTX-2.5-GGUF),
  [Deadshot699 Gemma4 NVFP4](https://huggingface.co/Deadshot699/ltx-2.5-gemma4-12b-comfy-nvfp4),
  [civitai LTX 2.3 8GB+32G](https://civitai.com/models/2477099/ltx-23-low-vram-8gb-32g-workflow),
  [NVIDIA RTX AI Garage](https://blogs.nvidia.com/blog/rtx-ai-garage-ces-2026-open-models-video-generation%27).
- Локально: `D:\AI\ComfyUI\ComfyUI\comfy\supported_models.py`, `comfy\quant_ops.py`,
  `comfy\ops.py`, `comfy\cli_args.py`, `comfy\storage.py`, `comfy\text_encoders\minimax.py`,
  `comfy_extras\nodes_minimax_h3.py`, `folder_paths.py`;
  `codewhale-desktop/src-tauri/src/comfy.rs`, `comfy_mcp.rs`;
  `codewhale-desktop/src-tauri/resources/comfy/README.md`; `docs/COMFYUI.md`.
