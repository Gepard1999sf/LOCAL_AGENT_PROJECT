import { test, expect } from "../mock/test";

async function openPanel(app: any, page: any) {
  await app.boot();
  await app.ready();
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Введите команду или название сессии…").fill("фабрика");
  await page.getByRole("option", { name: /Фабрика ассетов/ }).click();
  return page.locator("aside");
}

test.describe("Фабрика ассетов (ComfyUI)", () => {
  test("панель показывает, кто держит VRAM, и состояние ComfyUI", async ({ app, page }) => {
    const panel = await openPanel(app, page);
    await expect(panel.getByText("Фабрика ассетов", { exact: true })).toBeVisible();
    await expect(panel.getByTestId("vram-owner")).toContainText("VRAM: занята CodeWhale (qwen3:4b)");
    await expect(panel.getByText("ComfyUI не запущен")).toBeVisible();
    await expect(panel.getByText("Укажите папку ComfyUI с main.py")).toBeVisible();
    await expect(panel.getByRole("button", { name: "Запустить ComfyUI" })).toBeDisabled();
    await expect(panel.getByText("Шаблоны не установлены")).toBeVisible();
  });

  test("папка ComfyUI сохраняется в настройках; запуск и остановка меняют статус", async ({ app, backend, page }) => {
    const panel = await openPanel(app, page);
    const dir = panel.getByLabel("Папка ComfyUI");
    await dir.fill("D:\\AI\\ComfyUI");
    await dir.blur();
    await expect.poll(() => backend.settings.comfy?.dir).toBe("D:\\AI\\ComfyUI");
    expect(backend.settings.comfy?.unloadLlm).toBe(true);

    backend.comfy.dirValid = true;
    await panel.getByRole("button", { name: "Обновить" }).click();
    await panel.getByRole("button", { name: "Запустить ComfyUI" }).click();
    await expect(page.getByText("ComfyUI запущен")).toBeVisible();
    await expect(panel.getByText(/ComfyUI 0\.11\.86 отвечает \(запущен приложением\)/)).toBeVisible();
    await expect(panel.getByText(/свободно 7100 из 8192 МБ/)).toBeVisible();
    await panel.getByRole("button", { name: "Остановить ComfyUI" }).click();
    await expect(panel.getByText("ComfyUI не запущен")).toBeVisible();
  });

  test("установка шаблонов и подключение MCP", async ({ app, backend, page }) => {
    const panel = await openPanel(app, page);
    await panel.getByRole("button", { name: "Установить шаблоны" }).click();
    await expect(panel.getByText(/Шаблонов: ui_icon, pbr_material, sprite_sheet/)).toBeVisible();
    await expect(panel.getByText("Скилл comfyui-gamedev установлен")).toBeVisible();
    await expect(panel.getByText("ui_icon", { exact: true })).toBeVisible();

    await panel.getByRole("button", { name: "Подключить MCP" }).click();
    await expect(page.getByText("MCP-сервер comfy подключён, CodeWhale перезапущен")).toBeVisible();
    const cmds = backend.invokes.map((i) => i.cmd);
    expect(cmds.indexOf("runtime_restart")).toBeGreaterThan(cmds.indexOf("comfy_register_mcp"));
    await expect(panel.getByRole("button", { name: "Отключить MCP" })).toBeVisible();

    await panel.getByLabel("Чекпоинт").selectOption("sdxl_base.safetensors");
    await expect.poll(() => backend.settings.comfy?.checkpoint).toBe("sdxl_base.safetensors");
  });
});
