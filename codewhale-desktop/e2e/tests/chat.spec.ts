import { test, expect } from "../mock/test";

const LITE_TOOLS = ["read", "write", "edit", "bash", "todo_write"];

test.describe("Чат", () => {
  test("новая сессия и потоковый ответ модели", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();

    const answer = "Привет! Я **CodeWhale**, локальный агент. Чем помочь с проектом?";
    backend.scriptNextTurn(async (t) => {
      await t.start();
      await t.message(answer, { chunk: 8, delayMs: 10, pauseAfterFirstDelta: "mid-stream" });
      await t.complete({ input_tokens: 3200, output_tokens: 18 });
    });

    const threadId = await app.newSession();
    const create = backend.requests("POST", "/v1/threads")[0];
    expect(create.body).toMatchObject({
      model_provider: "ollama",
      model: "qwen3-8b-16k",
      workspace: "C:/Users/dev/projects/demo",
      allowed_tools: LITE_TOOLS,
    });

    await app.send("Привет");
    await expect.poll(() => backend.requests("POST", `/v1/threads/${threadId}/turns`).length).toBe(1);
    const turnReq = backend.requests("POST", `/v1/threads/${threadId}/turns`)[0];
    expect(turnReq.body.prompt).toBe("Привет");
    expect(turnReq.body.operation_key).toMatch(/^desk-/);
    expect(turnReq.body.allowed_tools).toEqual(LITE_TOOLS);

    // The user bubble, then the first delta with the streaming cursor while the turn is running.
    await expect(app.userBubbles.filter({ hasText: "Привет" })).toBeVisible();
    const stream = app.main.locator(".md.stream-cursor");
    await expect(stream).toContainText("Привет! ");
    await expect(page.locator("footer")).toContainText("Думает");
    await expect(app.main.getByText("Стоп (Esc)")).toBeVisible();

    backend.release("mid-stream");

    // Full markdown answer, cursor gone, per-turn stats rendered.
    await expect(app.main.locator(".md strong", { hasText: "CodeWhale" })).toBeVisible();
    await expect(app.main.locator(".md").last()).toContainText("Чем помочь с проектом?");
    await expect(stream).toHaveCount(0);
    await expect(app.main.getByText("Стоп (Esc)")).toHaveCount(0);
    await expect(app.main.getByText(/выход 18/)).toBeVisible();
    await expect(page.locator("footer")).toContainText("Готов");
    // The composer is cleared and usable again.
    await expect(app.composer).toHaveValue("");
  });

  test("отправка без открытой сессии создаёт её автоматически", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    await app.send("Сколько файлов в проекте?");
    await expect.poll(() => backend.requests("POST", "/v1/threads").length).toBe(1);
    await expect(app.main.getByText("Готово. Вы написали: «Сколько файлов в проекте?».")).toBeVisible();
  });

  test("Стоп прерывает ход: запрос interrupt и пометка «Остановлено»", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(async (t) => {
      await t.start();
      await t.message("Начинаю долгий ответ… ".repeat(4), { chunk: 6, delayMs: 10, pauseAfterFirstDelta: "never" });
      await t.complete();
    });
    const threadId = await app.newSession();
    await app.send("Расскажи длинную историю");
    await expect(app.main.locator(".md.stream-cursor")).toBeVisible();

    await app.main.getByText("Стоп (Esc)").click();
    await expect.poll(() => backend.requests("POST", new RegExp(`^/v1/threads/${threadId}/turns/[^/]+/interrupt$`)).length).toBe(1);
    await expect(app.main.getByText("Остановлено")).toBeVisible();
    await expect(app.main.locator(".md.stream-cursor")).toHaveCount(0);
  });

  test("ошибка хода показывает блок ошибки с кнопкой «Повторить»", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(async (t) => {
      await t.start();
      await t.fail("Network error: Cannot reach localhost:11434 (connection refused)");
    });
    await app.newSession();
    await app.send("Привет");
    await expect(app.main.getByText("Ошибка").first()).toBeVisible();
    await expect(app.main.getByText(/Cannot reach localhost:11434/).first()).toBeVisible();
    await expect(app.main.getByRole("button", { name: "Повторить" })).toBeVisible();
  });

  test(
    "прерванные размышления не остаются в состоянии «Думаю…» (item.interrupted)",
    async ({ app, backend }) => {
      // The runtime ends streaming items of an interrupted turn with `item.interrupted`
      // (fixtures/live/mock_14_turn_interrupt__sse.json); the reasoning block must leave its in-progress state.
      await app.boot();
      await app.ready();
      backend.scriptNextTurn(async (t) => {
        await t.start();
        await t.reasoning("Думаю над задачей… ".repeat(3), { chunk: 5, delayMs: 10, pauseAfterFirstDelta: "never" });
        await t.complete();
      });
      await app.newSession();
      await app.send("Подумай");
      await expect(app.main.getByText("Думаю…")).toBeVisible();
      await app.main.getByText("Стоп (Esc)").click();
      await expect(app.main.getByText("Остановлено")).toBeVisible();
      await expect(app.main.getByText("Думаю…")).toHaveCount(0, { timeout: 3000 });
    },
  );
});
