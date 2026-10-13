import { test, expect } from "../mock/test";

test.describe("Палитра команд и настройки", () => {
  test("Ctrl+K открывает палитру, поиск и выбор команды", async ({ app, page }) => {
    await app.boot();
    await app.ready();

    await page.keyboard.press("Control+k");
    const input = page.getByPlaceholder("Введите команду или название сессии…");
    await expect(input).toBeVisible();
    await expect(input).toBeFocused();
    await expect(page.getByText("Действия")).toBeVisible();
    await expect(page.getByRole("option", { name: /Новая сессия/ }).first()).toBeVisible();

    // Fuzzy search narrows the list; Enter runs the selected command.
    await input.fill("настройки");
    await expect(page.getByRole("option", { name: /Настройки/ })).toBeVisible();
    await expect(page.getByRole("option", { name: /Перезапустить CodeWhale/ })).toHaveCount(0);
    await page.keyboard.press("Enter");
    await expect(input).toHaveCount(0);
    await expect(page.getByText("⚙ Настройки")).toBeVisible();
  });

  test("Esc и повторное Ctrl+K закрывают палитру; пустой поиск пишет «Ничего не найдено»", async ({ app, page }) => {
    await app.boot();
    await app.ready();
    const input = page.getByPlaceholder("Введите команду или название сессии…");

    await page.keyboard.press("Control+k");
    await expect(input).toBeVisible();
    await input.fill("zzzz-нет-такой-команды");
    await expect(page.getByText("Ничего не найдено")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(input).toHaveCount(0);

    await page.keyboard.press("Control+k");
    await expect(input).toBeVisible();
    await page.keyboard.press("Control+k");
    await expect(input).toHaveCount(0);
  });

  test("палитра переключает модель (PATCH треда и settings_set)", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();
    const threadId = await app.newSession();
    await page.keyboard.press("Control+k");
    await page.getByPlaceholder("Введите команду или название сессии…").fill("gemma3");
    await page.getByRole("option", { name: /gemma3:4b/ }).click();
    await expect.poll(() => backend.settings.defaultModel).toBe("gemma3:4b");
    await expect.poll(() => backend.requests("PATCH", `/v1/threads/${threadId}`)[0]?.body).toEqual({
      model: "gemma3:4b",
      model_provider: "ollama",
    });
    await expect(page.getByText("Модель: gemma3:4b")).toBeVisible();
  });

  test("панель настроек открывается кнопкой, переключатель сохраняет настройку", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();

    await page.getByRole("button", { name: "Настройки (Ctrl+,)" }).click();
    await expect(page.getByText("⚙ Настройки")).toBeVisible();
    for (const section of ["Ollama", "CodeWhale", "Внешний вид", "Агент"]) {
      await expect(page.getByText(section, { exact: true }).first()).toBeVisible();
    }

    const label = page.locator("label", { hasText: "Показывать вызовы инструментов" });
    const sw = label.getByRole("switch");
    await expect(sw).toHaveAttribute("aria-checked", "true");
    await sw.click();
    await expect.poll(() => backend.calls("settings_set").map((c) => c.args.patch)).toContainEqual({ showToolCalls: false });
    await expect(sw).toHaveAttribute("aria-checked", "false");
    expect(backend.settings.showToolCalls).toBe(false);

    await page.getByRole("button", { name: "Закрыть (Esc)" }).click();
    await expect(page.getByText("⚙ Настройки")).toHaveCount(0);
  });

  test("Ctrl+, открывает и закрывает настройки, Esc закрывает", async ({ app, page }) => {
    await app.boot();
    await app.ready();
    await page.keyboard.press("Control+Comma");
    await expect(page.getByText("⚙ Настройки")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByText("⚙ Настройки")).toHaveCount(0);
  });

  test("Ctrl+/ показывает горячие клавиши", async ({ app, page }) => {
    await app.boot();
    await app.ready();
    await page.keyboard.press("Control+Slash");
    await expect(page.getByText("⌨ Горячие клавиши")).toBeVisible();
    await expect(page.getByText("Палитра команд")).toBeVisible();
  });
});
