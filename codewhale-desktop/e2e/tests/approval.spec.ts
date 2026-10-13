import { test, expect } from "../mock/test";
import { HttpError } from "../mock/backend";
import type { TurnScript } from "../mock/turn";

const bashTurn = (approvalId: string): TurnScript => async (t) => {
  await t.start();
  await t.message("Запущу тесты, чтобы проверить сборку.");
  const decision = await t.tool({
    name: "bash",
    args: { command: "cargo test --workspace" },
    output: "test result: ok. 12 passed; 0 failed",
    approval: { id: approvalId, summary: "Run `cargo test --workspace`", intent: "Проверить, что тесты проходят после правки." },
  });
  await t.message(decision === "allow" ? "Все 12 тестов прошли." : "Хорошо, не запускаю команду.");
  await t.complete();
};

test.describe("Подтверждение инструментов", () => {
  test("approval.required показывает карточку, «Разрешить» отправляет POST /v1/approvals/{id}", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(bashTurn("approval_e2e_allow"));
    await app.newSession();
    await app.send("Прогони тесты");

    const card = app.main.locator("div", { hasText: "Агент просит разрешение" }).filter({ has: page.getByRole("button", { name: "Разрешить" }) }).last();
    await expect(card).toBeVisible();
    await expect(card).toContainText("Run `cargo test --workspace`");
    await expect(card).toContainText("cargo test --workspace");
    await expect(card).toContainText("bash");
    await expect(card).toContainText("Проверить, что тесты проходят после правки.");
    await expect(page.locator("footer")).toContainText("Ждёт решения");
    // bash is not auto-approved by default: nothing was sent yet.
    expect(backend.requests("POST", /^\/v1\/approvals\//)).toHaveLength(0);

    await card.getByRole("button", { name: "Разрешить" }).click();

    await expect.poll(() => backend.requests("POST", "/v1/approvals/approval_e2e_allow").length).toBe(1);
    expect(backend.requests("POST", "/v1/approvals/approval_e2e_allow")[0].body).toEqual({ decision: "allow", remember: false });
    // The same call through the Tauri command, as Rust receives it.
    const ipc = backend.calls("cw_request").find((c) => c.args.path === "/v1/approvals/approval_e2e_allow");
    expect(ipc?.args).toMatchObject({ method: "POST", body: { decision: "allow", remember: false } });

    await expect(app.main.getByText("Разрешено:")).toBeVisible();
    await expect(app.main.getByRole("button", { name: "Разрешить" })).toHaveCount(0);
    await expect(app.main.getByText("Все 12 тестов прошли.")).toBeVisible();
    await expect(page.locator("footer")).toContainText("Готов");
  });

  test("«Отклонить» отправляет decision=deny", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(bashTurn("approval_e2e_deny"));
    await app.newSession();
    await app.send("Прогони тесты");
    await app.main.getByRole("button", { name: "Отклонить" }).click();
    await expect.poll(() => backend.requests("POST", "/v1/approvals/approval_e2e_deny")[0]?.body).toEqual({
      decision: "deny",
      remember: false,
    });
    await expect(app.main.getByText("Отклонено:")).toBeVisible();
    await expect(app.main.getByText("Хорошо, не запускаю команду.")).toBeVisible();
  });

  test("«Всегда в этой сессии» отправляет remember=true", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(bashTurn("approval_e2e_remember"));
    await app.newSession();
    await app.send("Прогони тесты");
    await app.main.getByRole("button", { name: "Всегда в этой сессии" }).click();
    await expect.poll(() => backend.requests("POST", "/v1/approvals/approval_e2e_remember")[0]?.body).toEqual({
      decision: "allow",
      remember: true,
    });
  });

  test("инструменты из автоподтверждения (read) одобряются без клика", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(async (t) => {
      await t.start();
      await t.tool({ name: "read", args: { path: "src/main.rs" }, output: "fn main() {}", approval: { id: "approval_e2e_auto" } });
      await t.message("Прочитал main.rs.");
      await t.complete();
    });
    await app.newSession();
    await app.send("Прочитай main.rs");
    await expect.poll(() => backend.requests("POST", "/v1/approvals/approval_e2e_auto")[0]?.body).toEqual({
      decision: "allow",
      remember: false,
    });
    await expect(app.main.getByText("Прочитал main.rs.")).toBeVisible();
  });

  test("запоздалый клик по закрытому запросу (HTTP 404) показывает «Запрос уже закрыт»", async ({ app, backend, page }) => {
    await app.boot();
    await app.ready();
    backend.scriptNextTurn(bashTurn("approval_e2e_gone"));
    // The runtime already settled the request (e.g. timeout) before the user clicks.
    backend.route("POST", /^\/v1\/approvals\/approval_e2e_gone$/, () => {
      throw new HttpError(404, "no pending approval with id 'approval_e2e_gone'");
    });
    await app.newSession();
    await app.send("Прогони тесты");
    await app.main.getByRole("button", { name: "Разрешить" }).click();
    // cw_request rejects with "HTTP 404: …"; the store treats it as already settled.
    await expect(page.getByText("Запрос уже закрыт")).toBeVisible();
    await expect(page.getByText("HTTP 404: no pending approval with id 'approval_e2e_gone'")).toBeVisible();
  });
});
