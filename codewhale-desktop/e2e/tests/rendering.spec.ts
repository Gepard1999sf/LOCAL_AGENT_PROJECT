import { test, expect } from "../mock/test";
import { SAMPLE_DIFF } from "../mock/events";

const RUST = ["```rust", "fn main() {", '    let greeting = "Привет";', '    println!("{greeting}, мир!");', "}", "```"].join("\n");

/** Seeds a finished thread with one answer and opens it from the sidebar. */
async function openAnswer(app: import("../mock/test").AppDriver, title: string, answer: string, extra?: (t: import("../mock/turn").TurnCtx) => Promise<void>) {
  await app.backend.seedThread({
    title,
    turns: [
      {
        prompt: "Покажи пример",
        script: async (t) => {
          await t.start();
          if (extra) await extra(t);
          await t.message(answer);
          await t.complete();
        },
      },
    ],
  });
  await app.boot();
  await app.ready();
  await app.page.locator("aside").first().getByText(title).click();
  await expect(app.main.getByText("Покажи пример").last()).toBeVisible();
}

test.describe("Рендеринг ответов", () => {
  test("блок кода в markdown подсвечивается Shiki и копируется", async ({ app, page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await openAnswer(app, "Пример на Rust", `Вот программа:\n\n${RUST}\n\nЗапустите \`cargo run\`.`);

    const block = app.main.locator(".code-block").first();
    await expect(block).toBeVisible();
    await expect(block).toHaveAttribute("data-code", /fn main\(\)/);
    await expect(block.getByText("rust", { exact: true })).toBeVisible();
    // Shiki output: <pre class="shiki ..."> with dual-theme token colors in CSS variables.
    const token = block.locator('pre.shiki span[style*="--shiki-dark"]').first();
    await expect(token).toBeAttached({ timeout: 10_000 });
    expect(await block.locator('pre.shiki span[style*="--shiki-dark"]').count()).toBeGreaterThan(3);
    await expect(block).toContainText('println!("{greeting}, мир!");');
    // Inline code stays inline.
    await expect(app.main.locator(".md code", { hasText: "cargo run" })).toBeVisible();

    await block.getByRole("button", { name: "Копировать" }).click();
    await expect(block.getByText("Скопировано")).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toContain("fn main()");
  });

  test("markdown: таблицы GFM, списки и ссылки", async ({ app, backend }) => {
    const md = [
      "## План",
      "",
      "1. Прочитать `Cargo.toml`",
      "2. Запустить тесты",
      "",
      "| Файл | Строк |",
      "| --- | ---: |",
      "| src/main.rs | 42 |",
      "",
      "Документация: [Ollama](https://docs.ollama.com)",
    ].join("\n");
    await openAnswer(app, "Markdown", md);
    await expect(app.main.locator(".md h2", { hasText: "План" })).toBeVisible();
    await expect(app.main.locator(".md ol li")).toHaveCount(2);
    await expect(app.main.locator(".md table td", { hasText: "src/main.rs" })).toBeVisible();
    await app.main.locator(".md a", { hasText: "Ollama" }).click();
    await expect.poll(() => backend.calls("plugin:opener|open_url").map((c) => c.args.url)).toContain("https://docs.ollama.com");
  });

  test("diff в markdown рендерится как unified diff с номерами строк и режимом «Бок о бок»", async ({ app }) => {
    await openAnswer(app, "Правка main.rs", `Изменение:\n\n\`\`\`diff\n${SAMPLE_DIFF}\n\`\`\``);

    const block = app.main.locator(".code-block").first();
    await expect(block.getByText("diff", { exact: true })).toBeVisible();
    const rows = block.locator("table tbody tr");
    await expect(rows).toHaveCount(9);
    await expect(block.getByText("+2", { exact: true })).toBeVisible();
    await expect(block.getByText("−1", { exact: true })).toBeVisible();

    const added = rows.filter({ hasText: 'let name = "CodeWhale";' });
    await expect(added).toHaveCount(1);
    await expect(added).toHaveClass(/diff-add/);
    await expect(added.locator("td").nth(1)).toHaveText("2"); // new line number
    const removed = rows.filter({ hasText: 'println!("Hello, world!");' });
    await expect(removed).toHaveClass(/diff-del/);
    await expect(removed.locator("td").first()).toHaveText("2"); // old line number
    await expect(rows.filter({ hasText: "@@ -1,4 +1,5 @@" })).toHaveCount(1);

    await block.getByRole("button", { name: "Бок о бок" }).click();
    await expect(block.getByRole("button", { name: "Единый вид" })).toBeVisible();
    await expect(block.locator("table.table-fixed")).toBeVisible();
  });

  test("правка файла (file_change с metadata.mutation.diff) показывает дифф в блоке инструмента", async ({ app }) => {
    await openAnswer(app, "Агент правит файл", "Файл обновлён.", async (t) => {
      await t.tool({
        name: "edit",
        args: { path: "src/main.rs", old_string: 'println!("Hello, world!");', new_string: 'println!("Hello, {name}!");' },
        output: "Applied 1 edit to src/main.rs",
        diff: SAMPLE_DIFF,
        files: [{ path: "src/main.rs", outcome: "modified" }],
      });
    });

    const tool = app.main.locator("button", { hasText: "edit" }).filter({ hasText: "src/main.rs" }).first();
    await expect(tool).toBeVisible();
    await expect(tool).toContainText("+2");
    await expect(tool).toContainText("−1");
    await tool.click();
    const table = app.main.locator("table").first();
    await expect(table).toBeVisible();
    await expect(table.locator("tr", { hasText: 'let name = "CodeWhale";' })).toHaveClass(/diff-add/);
    await expect(app.main.getByText("Applied 1 edit to src/main.rs")).toBeVisible();
  });
});
