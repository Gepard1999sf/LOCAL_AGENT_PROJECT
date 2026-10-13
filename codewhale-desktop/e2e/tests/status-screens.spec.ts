import { test, expect } from "../mock/test";

test.describe("Экраны состояния", () => {
  test("runtime в состоянии failed: экран ошибки, перезапуск возвращает чат", async ({ app, backend, page }) => {
    backend.runtime = {
      state: "failed",
      port: null,
      pid: null,
      binary: null,
      version: null,
      error: "Не найден codewhale.exe. Укажите путь в настройках.",
      startedAtMs: null,
    };
    backend.logs = ["[desktop] spawn codewhale.exe: program not found"];
    await app.boot();

    await expect(page.getByRole("heading", { name: "CodeWhale не запустился" })).toBeVisible();
    await expect(page.getByText("Не найден codewhale.exe. Укажите путь в настройках.")).toBeVisible();
    await expect(page.getByText("spawn codewhale.exe: program not found")).toBeVisible();
    await expect(page.locator("footer")).toContainText("CodeWhale не запущен");
    // No runtime: the UI must not call the HTTP API.
    expect(backend.requests()).toHaveLength(0);

    await page.getByRole("button", { name: "Перезапустить" }).click();
    await expect.poll(() => backend.calls("runtime_restart").length).toBe(1);
    await expect(page.getByRole("heading", { name: "Добро пожаловать в CodeWhale Desktop" })).toBeVisible();
    // Becoming "running" refreshes the thread list.
    await expect.poll(() => backend.requests("GET", /^\/v1\/threads\/summary/).length).toBeGreaterThan(0);
  });

  test("падение runtime во время работы (событие runtime-status) показывает экран ошибки", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();
    await expect(page.getByRole("heading", { name: "Добро пожаловать в CodeWhale Desktop" })).toBeVisible();

    await backend.setRuntime({ state: "failed", error: "codewhale.exe завершился с кодом 101" });
    await expect(page.getByRole("heading", { name: "CodeWhale не запустился" })).toBeVisible();
    await expect(page.getByText("codewhale.exe завершился с кодом 101")).toBeVisible();

    await page.getByRole("button", { name: "Указать путь к CodeWhale" }).click();
    await expect(page.getByText("⚙ Настройки")).toBeVisible();
  });

  test("runtime ещё запускается: статус «Запуск CodeWhale», отправка блокируется", async ({ app, backend, page }) => {
    backend.runtime = { ...backend.runtime, state: "starting" };
    await app.boot();
    await expect(page.locator("footer")).toContainText("Запуск CodeWhale");
    await app.send("Привет");
    await expect(page.getByText("CodeWhale ещё запускается, подождите")).toBeVisible();
    expect(backend.requests("POST", /\/turns$/)).toHaveLength(0);

    await backend.setRuntime({ state: "running" });
    await expect(page.locator("footer")).not.toContainText("Запуск CodeWhale");
  });

  test("Ollama не запущена: экран с подсказкой, «Проверить снова» после запуска", async ({ app, backend, page }) => {
    backend.setOllamaOnline(false);
    await app.boot();

    await expect(page.getByRole("heading", { name: "Ollama не запущена" })).toBeVisible();
    await expect(page.getByText("http://127.0.0.1:11434").first()).toBeVisible();
    await expect(page.getByText("ollama serve")).toBeVisible();
    await expect(page.locator("footer")).toContainText("Ollama offline");

    backend.setOllamaOnline(true);
    await page.getByRole("button", { name: "Проверить снова" }).click();
    await expect(page.getByRole("heading", { name: "Добро пожаловать в CodeWhale Desktop" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Ollama не запущена" })).toHaveCount(0);
  });

  test("Ollama пропадает во время работы: автоопрос каждые 5 с переключает экран", async ({ app, backend, page }) => {
    await page.clock.install();
    await app.boot();
    await app.ready();
    backend.setOllamaOnline(false);
    await page.clock.fastForward(5_500);
    await expect(page.getByRole("heading", { name: "Ollama не запущена" })).toBeVisible();
    backend.setOllamaOnline(true);
    await page.clock.fastForward(5_500);
    await expect(page.getByRole("heading", { name: "Добро пожаловать в CodeWhale Desktop" })).toBeVisible();
  });
});
