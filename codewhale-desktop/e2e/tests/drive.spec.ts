import { test, expect } from "../mock/test";

test.describe("Диск знаний", () => {
  test("панель открывается из палитры и показывает разделы диска", async ({ app, page }) => {
    await app.boot();
    await app.ready();

    await page.keyboard.press("Control+k");
    await page.getByPlaceholder("Введите команду или название сессии…").fill("диск");
    await page.getByRole("option", { name: /Диск знаний/ }).click();

    const panel = page.locator("aside");
    await expect(panel.getByText("Диск знаний", { exact: true })).toBeVisible();
    await expect(panel.getByText("Диск подключён")).toBeVisible();
    await expect(panel.getByText(/docsets — Оффлайн-документация/)).toBeVisible();
    await expect(panel.getByText(/math_physics — /)).toBeVisible();
    await expect(panel.getByText("MCP-сервер не подключён")).toBeVisible();
    await expect(panel.getByText("Индекс не построен — code_search недоступен")).toBeVisible();
    await expect(panel.getByText("qwen2.5-coder:7b")).toBeVisible();
  });

  test("«Подключить MCP» регистрирует сервер и перезапускает CodeWhale", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();
    await page.keyboard.press("Control+k");
    await page.getByPlaceholder("Введите команду или название сессии…").fill("диск");
    await page.getByRole("option", { name: /Диск знаний/ }).click();
    const panel = page.locator("aside");

    await panel.getByRole("button", { name: "Подключить MCP" }).click();
    await expect(page.getByText("MCP-сервер ai-drive подключён, CodeWhale перезапущен")).toBeVisible();
    await expect(panel.getByText("MCP-сервер ai-drive зарегистрирован")).toBeVisible();
    const cmds = backend.invokes.map((i) => i.cmd);
    expect(cmds.indexOf("drive_register_mcp")).toBeGreaterThanOrEqual(0);
    expect(cmds.indexOf("runtime_restart")).toBeGreaterThan(cmds.indexOf("drive_register_mcp"));
    await expect(panel.getByRole("button", { name: "Отключить MCP" })).toBeVisible();
  });

  test("индекс, Kiwix и скиллы отражают ответы бэкенда", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();
    await page.keyboard.press("Control+k");
    await page.getByPlaceholder("Введите команду или название сессии…").fill("диск");
    await page.getByRole("option", { name: /Диск знаний/ }).click();
    const panel = page.locator("aside");

    await panel.getByRole("button", { name: "Построить индекс" }).click();
    await expect(page.getByText("Индекс git_vault построен")).toBeVisible();
    await expect(panel.getByText(/Индекс построен: 1234 файлов/)).toBeVisible();

    await panel.getByRole("button", { name: "Запустить Kiwix" }).click();
    await expect(panel.getByText(/kiwix-serve отвечает на http:\/\/127\.0\.0\.1:8090/)).toBeVisible();
    await expect(panel.getByText("gamedev.stackexchange.com_en_all", { exact: true })).toBeVisible();
    await panel.getByRole("button", { name: "Остановить Kiwix" }).click();
    await expect(panel.getByText(/kiwix-serve не запущен/)).toBeVisible();

    await panel.getByRole("button", { name: "Установить скиллы" }).click();
    await expect(panel.getByText(/Скиллы установлены: knowledge-drive, math-physics-godot/)).toBeVisible();
    expect(backend.invokes.find((i) => i.cmd === "drive_install_skills")?.args).toEqual({ toWorkspace: false });

    // Settings: the drive root typed in the panel is saved under knowledgeDrive.root.
    const rootInput = panel.getByLabel("Папка диска");
    await rootInput.fill("E:\\AI_Drive");
    await rootInput.blur();
    await expect.poll(() => backend.settings.knowledgeDrive?.root).toBe("E:\\AI_Drive");
  });
});
