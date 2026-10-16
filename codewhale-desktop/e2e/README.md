# E2E-тесты интерфейса (Playwright, без Tauri и CodeWhale)

Тесты открывают настоящий React-интерфейс CodeWhale Desktop в headless Chromium.
Rust-часть (Tauri), `codewhale app-server` и Ollama подменены заглушками, поэтому тесты
идут за полминуты на любой машине, включая CI на Linux.

```
Vite dev server (npx vite --port 1420)  ->  Chromium  ->  window.__TAURI_INTERNALS__ (заглушка)
                                                              |  invoke(cmd, args)
                                                              v
                                         Playwright binding __cwMockInvoke  ->  MockBackend (Node)
                                                              ^
                                     события cw-event / runtime-status / runtime-log / cw-stream
```

## Запуск

```powershell
cd e2e
npm install          # один раз; ставит только @playwright/test и typescript
npx playwright install chromium   # один раз, если браузер Playwright ещё не скачан
npm test             # все тесты
```

Из корня проекта (после добавления скрипта `e2e` в корневой `package.json`):

```powershell
npm run e2e
```

Полезные варианты:

| Команда | Что делает |
|---|---|
| `npm test -- tests/chat.spec.ts` | один файл |
| `npm test -- -g "Ctrl+K"` | тесты по имени |
| `npm run test:headed` | с видимым окном браузера |
| `npm run test:ui` | интерактивный режим Playwright UI |
| `npm run report` | открыть HTML-отчёт последнего прогона |
| `npm run typecheck` | проверка типов заглушек и тестов |
| `npx playwright show-trace test-results/<тест>/trace.zip` | трасса упавшего теста (шаги, DOM, сеть, консоль) |

Если `npm run tauri dev` уже держит порт 1420, Playwright использует этот Vite-сервер
(кроме CI). Переменные окружения:

| Переменная | Значение |
|---|---|
| `E2E_PORT=1430` | другой порт для Vite, если 1420 занят чем-то посторонним |
| `CW_E2E_CHROMIUM=C:\path\chrome.exe` | свой бинарник Chromium вместо браузера Playwright |
| `CW_E2E_FIXTURES=synthetic` | не использовать `fixtures/live`, только синтетические данные |
| `CW_E2E_ALLOW_PAGE_ERRORS=1` | не валить тест из-за необработанных исключений на странице |
| `CI=1` | 2 воркера, 1 повтор, запрет `test.only`, свежий Vite-сервер |

## Что проверяется

| Файл | Сценарии |
|---|---|
| `tests/boot.spec.ts` | запуск до основного макета, выбор модели при старте, открытие недавней сессии из боковой панели |
| `tests/chat.spec.ts` | новая сессия, потоковый ответ из `item.delta` (курсор, статус «Думает», итоговая статистика), автосоздание сессии, «Стоп» → interrupt, ошибка хода с «Повторить» |
| `tests/approval.spec.ts` | карточка `approval.required`; «Разрешить» / «Отклонить» / «Всегда в этой сессии» → `cw_request POST /v1/approvals/{id}`; автоподтверждение `read`; ответ 404 на закрытый запрос |
| `tests/status-screens.spec.ts` | runtime `failed` (экран ошибки, перезапуск), падение runtime через событие, состояние `starting`, Ollama недоступна и пропадает во время работы |
| `tests/overlays.spec.ts` | палитра команд (Ctrl+K, поиск, Esc, смена модели), панель настроек (кнопка, Ctrl+,, переключатель → `settings_set`), справка по клавишам (Ctrl+/) |
| `tests/models.spec.ts` | менеджер моделей Ollama: вкладки, оценка VRAM (`models_estimate_vram`), «Подробнее» (`models_show`), смена модели по умолчанию, удаление и копирование (`models_delete`/`models_copy`), загрузка с прогрессом через `Channel` и отмена (`models_pull`/`models_pull_cancel`), вкладка «Запущенные» (`models_running`, `ollama_unload`) |
| `tests/rendering.spec.ts` | подсветка кода Shiki и копирование, таблицы/списки/ссылки GFM, unified diff (номера строк, «Бок о бок»), дифф правки файла в блоке инструмента |

Тест «прерванные размышления не остаются в состоянии „Думаю…“» помечен `test.fail`:
он фиксирует известный пробел в `src/store/threads.ts` (нет обработки `item.interrupted`).
Когда пробел закроют, Playwright сообщит, что тест неожиданно прошёл, и пометку нужно снять.

## Устройство

```
e2e/
  playwright.config.ts     webServer = Vite приложения, Chromium, locale ru-RU
  mock/
    tauri-shim.ts          window.__TAURI_INTERNALS__ для @tauri-apps/api v2: invoke, transformCallback,
                           plugin:event|listen/unlisten/emit, convertFileSrc, metadata
    backend.ts             MockBackend: все команды src/lib/tauri.ts, плагины dialog/opener,
                           HTTP API runtime за cw_request, поток событий за cw_subscribe (с since_seq)
    turn.ts                сценарии ходов: start / message (дельты) / reasoning / tool (+approval) /
                           askUser / fail / interrupt / complete
    events.ts              конверты cw-event и записи runtime (форма как в runtime_api.rs)
    fixtures.ts            загрузка записанных ответов: fixtures/live лучше fixtures/synthetic
    defaults.ts            ответы Rust-команд (настройки, статус runtime, Ollama, GPU, диск знаний,
                           ComfyUI, архитектуры моделей и слои для models_*)
    test.ts                фикстуры Playwright `backend` и `app` (AppDriver)
  fixtures/
    live/                  ответы настоящего `codewhale app-server --http` (record_live.py)
    synthetic/             запасные ответы, написанные по структурам сервера
  tests/*.spec.ts
```

Каждый тест получает свой `MockBackend`: треды, ходы, журнал событий с `seq`, ожидающие
подтверждения. Ответы HTTP API берутся из шаблонов записанных фикстур, поэтому их форма
совпадает с настоящим сервером. Если `fixtures/live` нет или файл записан наполовину,
используется `fixtures/synthetic`.

## Как написать тест

```ts
import { test, expect } from "../mock/test";

test("агент правит файл после подтверждения", async ({ app, backend, page }) => {
  await app.boot();      // приложение смонтировано, слушатели событий зарегистрированы
  await app.ready();     // модель выбрана

  backend.scriptNextTurn(async (t) => {
    await t.start();
    await t.message("Сейчас поправлю.", { pauseAfterFirstDelta: "mid" });
    const decision = await t.tool({
      name: "edit",
      args: { path: "src/main.rs" },
      approval: { id: "appr_1", summary: "Edit src/main.rs" },
    });
    await t.message(decision === "allow" ? "Готово." : "Отменено.");
    await t.complete();
  });

  await app.newSession();
  await app.send("Поправь main.rs");
  backend.release("mid");                         // открыть «ворота» сценария
  await app.main.getByRole("button", { name: "Разрешить" }).click();
  await expect.poll(() => backend.requests("POST", "/v1/approvals/appr_1").length).toBe(1);
});
```

Основное API:

- `backend.scriptNextTurn(fn)`: сценарий следующего `POST /v1/threads/{id}/turns`;
  без него модель отвечает «Готово. Вы написали: …».
- `backend.seedThread({ title, turns })`: готовый тред с историей (виден в боковой панели).
- `backend.seedFromCapture(recording)`: тред из записанного SSE-потока `fixtures/live/*__sse.json`.
- `backend.setRuntime({ state: "failed", error })`, `backend.setOllamaOnline(false)`,
  `backend.log(line)`: состояние Rust-части и события `runtime-status` / `runtime-log`.
- `backend.emit(event, payload)`: любое Tauri-событие в страницу.
- `backend.onCommand(cmd, fn)`, `backend.route(method, /re/, fn)`: подмена команды или
  HTTP-маршрута; `throw new HttpError(404, "…")` даёт ошибку `HTTP 404: …`, как `cw_request`.
- `backend.calls(cmd)`, `backend.requests(method, path)`: журнал вызовов для проверок.
- `backend.unhandled`: неизвестные команды и маршруты (попадают в аннотации теста).
- `backend.pullStepMs`, `backend.pullHoldsAtLayer` + `backend.release("pull:<имя>")`: темп и пауза
  сценарной загрузки `models_pull` (прогресс уходит в `Channel` страницы через `runCallback`).

Если приложение не смонтировалось (например, Vite показал ошибку компиляции из-за
недописанного файла), `app.boot()` один раз перезагружает страницу, а при повторной
неудаче падает с текстом ошибки Vite и консоли.

## Новые команды Tauri

Новую команду в `src/lib/tauri.ts` нужно добавить в `MockBackend.builtin`
(`mock/backend.ts`). Новый маршрут HTTP API добавляется в `MockBackend.routes`.
Иначе вызов попадёт в `backend.unhandled` и вернёт `null` (или `HTTP 404`).
