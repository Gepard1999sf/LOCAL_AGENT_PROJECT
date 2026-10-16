import { test, expect, type AppDriver } from "../mock/test";
import type { Locator, Page } from "@playwright/test";

/** Boots the app and opens «Модели Ollama» from the command palette. Returns the panel. */
async function openModels(app: AppDriver, page: Page): Promise<Locator> {
  await app.boot();
  await app.ready();
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Введите команду или название сессии…").fill("models");
  await page.getByRole("option", { name: /Управление моделями Ollama/ }).click();
  const panel = page.locator("aside").filter({ hasText: "Модели Ollama" });
  await expect(panel).toBeVisible();
  return panel;
}

/** The card of an installed model (its name sits in a span with a title attribute). */
function modelRow(panel: Locator, name: string): Locator {
  return panel.locator(`div:has(> div > span[title="${name}"])`);
}

test.describe("Менеджер моделей Ollama", () => {
  test("вкладки, установленные модели, оценка VRAM и строка GPU", async ({ app, backend, page }) => {
    const panel = await openModels(app, page);

    await expect(panel.getByRole("tab", { name: /Установленные/ })).toHaveAttribute("data-state", "active");
    await expect(panel.getByRole("tab", { name: /Загрузить/ })).toBeVisible();
    await expect(panel.getByRole("tab", { name: /Запущенные/ })).toBeVisible();
    await expect(panel.getByText(/GPU: NVIDIA GeForce RTX 4070/)).toBeVisible();

    for (const name of ["qwen3-8b-16k", "qwen3:4b", "gemma3:4b", "nomic-embed-text"]) {
      await expect(modelRow(panel, name)).toBeVisible();
    }
    // The bootstrap default (first tools model with >= 16k context) is marked.
    await expect(modelRow(panel, "qwen3-8b-16k").getByText("по умолчанию")).toBeVisible();
    await expect(modelRow(panel, "qwen3-8b-16k").getByText(/загружена/)).toBeVisible();
    await expect(panel.getByText(/4 моделей/)).toBeVisible();

    // One VRAM estimate per non-embedding model, at its effective context
    // (num_ctx from the Modelfile, else Ollama's 4k tier for a 12 GB card).
    await expect.poll(() => backend.calls("models_estimate_vram").length).toBe(3);
    const asked = backend.calls("models_estimate_vram").map((c) => [c.args.name, c.args.numCtx]);
    expect(asked).toContainEqual(["qwen3-8b-16k", 16384]);
    expect(asked).toContainEqual(["qwen3:4b", 4096]);
    expect(asked).toContainEqual(["gemma3:4b", 4096]);
    await expect(modelRow(panel, "qwen3-8b-16k").getByText(/VRAM ≈/)).toBeVisible();
    await expect(modelRow(panel, "nomic-embed-text").getByText(/VRAM ≈/)).toHaveCount(0);
  });

  test("«Подробнее» запрашивает models_show и показывает архитектуру и таблицу KV-кэша", async ({ app, backend, page }) => {
    const panel = await openModels(app, page);
    const row = modelRow(panel, "qwen3:4b");
    await row.getByRole("button", { name: "Подробнее" }).click();
    // React StrictMode (dev build under Vite) runs the mount effect twice, so count ">= 1".
    await expect.poll(() => backend.calls("models_show").length).toBeGreaterThan(0);
    expect(new Set(backend.calls("models_show").map((c) => c.args.name))).toEqual(new Set(["qwen3:4b"]));
    await expect(row.getByText("Архитектура")).toBeVisible();
    await expect(row.getByText("qwen3", { exact: true }).first()).toBeVisible();
    await expect(row.getByText("Контекст обучения")).toBeVisible();
    await expect(row.getByText("Видеопамять при контексте")).toBeVisible();
    for (const label of ["f16 (по умолчанию)", "q8_0", "q4_0"]) {
      await expect(row.getByText(label, { exact: true })).toBeVisible();
    }
    // The estimate for the details table is a separate call at the selected context.
    await expect.poll(() => backend.calls("models_estimate_vram").filter((c) => c.args.name === "qwen3:4b").length).toBeGreaterThan(0);
  });

  test("«Использовать» делает модель моделью по умолчанию (settings_set)", async ({ app, backend, page }) => {
    const panel = await openModels(app, page);
    await modelRow(panel, "gemma3:4b").getByRole("button", { name: "Использовать" }).click();
    await expect.poll(() => backend.settings.defaultModel).toBe("gemma3:4b");
    await expect(modelRow(panel, "gemma3:4b").getByText("по умолчанию")).toBeVisible();
    await expect(modelRow(panel, "qwen3-8b-16k").getByText("по умолчанию")).toHaveCount(0);
  });

  test("удаление: подтверждение, models_delete, карточка исчезает", async ({ app, backend, page }) => {
    const panel = await openModels(app, page);
    const row = modelRow(panel, "qwen3:4b");
    await row.getByRole("button", { name: "Удалить" }).first().click();
    await expect(row.getByText("Удалить qwen3:4b?")).toBeVisible();
    await row.getByRole("button", { name: "Удалить" }).last().click();
    await expect.poll(() => backend.calls("models_delete").map((c) => c.args.name)).toEqual(["qwen3:4b"]);
    await expect(page.getByText("Модель qwen3:4b удалена")).toBeVisible();
    await expect(modelRow(panel, "qwen3:4b")).toHaveCount(0);
    expect(backend.models.map((m) => m.name)).not.toContain("qwen3:4b");
  });

  test("копирование создаёт копию через models_copy", async ({ app, backend, page }) => {
    const panel = await openModels(app, page);
    const row = modelRow(panel, "gemma3:4b");
    await row.getByRole("button", { name: "Копировать" }).first().click();
    const input = row.locator("input[type=text], input:not([type])").first();
    await expect(input).toHaveValue("gemma3:4b-copy");
    await row.getByRole("button", { name: "Копировать" }).last().click();
    await expect.poll(() => backend.calls("models_copy").map((c) => c.args)).toEqual([{ from: "gemma3:4b", to: "gemma3:4b-copy" }]);
    await expect(page.getByText("Создана копия gemma3:4b-copy")).toBeVisible();
    await expect(modelRow(panel, "gemma3:4b-copy")).toBeVisible();
  });

  test("загрузка: прогресс по слоям из Channel, затем модель в списке", async ({ app, backend, page }) => {
    backend.pullHoldsAtLayer = true;
    const panel = await openModels(app, page);
    await panel.getByRole("tab", { name: /Загрузить/ }).click();
    const input = panel.getByPlaceholder("qwen3:8b или hf.co/user/repo:Q4_K_M");
    await input.fill("qwen3:1.7b");
    await input.press("Enter");

    await expect.poll(() => backend.calls("models_pull").map((c) => c.args.name)).toEqual(["qwen3:1.7b"]);
    expect(backend.calls("models_pull")[0].args.onProgress).toMatch(/^__CHANNEL__:\d+$/);
    // Parked on the first layer: the row shows the stage, the byte counter and a cancel button.
    await expect(panel.getByText("загрузка слоёв")).toBeVisible();
    await expect(panel.getByText(/из 1[,.]3 GB/)).toBeVisible();
    await expect(panel.getByRole("button", { name: "Отменить загрузку" })).toBeVisible();
    // The recommended card of the same model reflects the download.
    await expect(panel.getByText("Загрузка", { exact: true })).toBeVisible();

    backend.release("pull:qwen3:1.7b");
    await expect(page.getByText("Модель qwen3:1.7b загружена")).toBeVisible();
    await expect(panel.getByText("готово", { exact: true })).toBeVisible();
    await expect(panel.getByRole("tab", { name: /Установленные/ })).toContainText("5");
    await expect(panel.getByText("Установлена", { exact: true })).toBeVisible();
    await expect.poll(() => backend.models.map((m) => m.name)).toContain("qwen3:1.7b");
  });

  test("отмена загрузки: models_pull_cancel, статус «отменено» и «Повторить»", async ({ app, backend, page }) => {
    backend.pullHoldsAtLayer = true;
    const panel = await openModels(app, page);
    await panel.getByRole("tab", { name: /Загрузить/ }).click();
    // «Скачать» on a recommended card starts the pull without typing a name.
    const card = panel.locator("div", { has: page.getByText("phi4-mini:3.8b", { exact: true }) }).last();
    await card.getByRole("button", { name: "Скачать" }).click();
    await expect.poll(() => backend.calls("models_pull").map((c) => c.args.name)).toEqual(["phi4-mini:3.8b"]);
    await expect(panel.getByRole("button", { name: "Отменить загрузку" })).toBeVisible();

    await panel.getByRole("button", { name: "Отменить загрузку" }).click();
    await expect.poll(() => backend.calls("models_pull_cancel").map((c) => c.args.name)).toEqual(["phi4-mini:3.8b"]);
    await expect(panel.getByText("отменено", { exact: true })).toBeVisible();
    await expect(panel.getByRole("button", { name: "Повторить" })).toBeVisible();
    await expect(panel.getByText(/Скачанные части сохранены/)).toBeVisible();
    expect(backend.models.map((m) => m.name)).not.toContain("phi4-mini:3.8b");
  });

  test("«Запущенные»: список из models_running, выгрузка через ollama_unload", async ({ app, backend, page }) => {
    const panel = await openModels(app, page);
    await panel.getByRole("tab", { name: /Запущенные/ }).click();
    await expect.poll(() => backend.calls("models_running").length).toBeGreaterThan(0);
    await expect(panel.getByText("Видеопамять NVIDIA GeForce RTX 4070")).toBeVisible();
    const row = panel.locator("div", { hasText: "qwen3-8b-16k:latest" }).last();
    await expect(row).toBeVisible();
    // 7.1 of 7.4 GB in VRAM: a few percent spilled to the CPU.
    await expect(panel.getByText("4% CPU / 96% GPU")).toBeVisible();
    await expect(panel.getByText(/Модель не поместилась в видеопамять/)).toBeVisible();
    await expect(panel.getByText("контекст 16k")).toBeVisible();

    await panel.getByRole("button", { name: "Выгрузить" }).click();
    await expect.poll(() => backend.calls("ollama_unload").map((c) => c.args.model)).toEqual(["qwen3-8b-16k:latest"]);
    await expect(page.getByText("qwen3-8b-16k:latest выгружена из памяти")).toBeVisible();
    await expect(panel.getByText(/Сейчас ни одна модель не загружена/)).toBeVisible();
  });
});
