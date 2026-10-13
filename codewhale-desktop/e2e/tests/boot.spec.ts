import { test, expect } from "../mock/test";

test.describe("Запуск", () => {
  test("приложение загружается в основной макет", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();

    // Toolbar, sidebar, chat area with the welcome screen, status bar.
    await expect(page.locator("header")).toContainText("CodeWhale");
    await expect(page.getByRole("button", { name: "Боковая панель (Ctrl+B)" })).toBeVisible();
    await expect(page.locator("aside").first()).toBeVisible();
    await expect(page.getByRole("heading", { name: "Добро пожаловать в CodeWhale Desktop" })).toBeVisible();
    await expect(app.composer).toBeVisible();
    await expect(page.locator("footer")).toContainText("Готов");

    // Bootstrap asked the runtime for threads and picked an agent-ready model.
    expect(backend.requests("GET", /^\/v1\/threads\/summary/).length).toBeGreaterThan(0);
    const set = backend.calls("settings_set").map((c) => c.args.patch);
    expect(set).toContainEqual({ defaultModel: "qwen3-8b-16k" });
    expect(backend.calls("ollama_models").length).toBeGreaterThan(0);
  });

  test("недавние сессии видны в боковой панели и открываются", async ({ app, backend, page }) => {
    const id = await backend.seedThread({
      title: "Разбор main.rs",
      turns: [{ prompt: "Что делает main.rs?", script: async (t) => {
        await t.start();
        await t.message("Файл `main.rs` запускает сервер.");
        await t.complete();
      } }],
    });
    await app.boot();
    await app.ready();

    const sidebar = page.locator("aside").first();
    await expect(sidebar.getByText("Разбор main.rs")).toBeVisible();
    await sidebar.getByText("Разбор main.rs").click();

    // Opening replays the journal through cw_subscribe.
    await expect.poll(() => backend.calls("cw_subscribe").map((c) => c.args.threadId)).toContain(id);
    await expect(app.main.getByText("Что делает main.rs?")).toBeVisible();
    await expect(app.main.getByText("запускает сервер")).toBeVisible();
  });
});
