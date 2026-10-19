import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Locator, Page } from "@playwright/test";
import { test, expect, type AppDriver } from "../mock/test";

const here = path.dirname(fileURLToPath(import.meta.url));
/** The CSP of the Tauri window (src-tauri/tauri.conf.json), so the test follows the real config. */
const APP_CSP: string = JSON.parse(fs.readFileSync(path.resolve(here, "../../src-tauri/tauri.conf.json"), "utf8")).app.security.csp;

const fence = (...lines: string[]) => ["```mermaid", ...lines, "```"].join("\n");

const FLOW = fence(
  "flowchart LR",
  '  input["Ввод: x = 5"] --> store[("Переменная x")]',
  '  store --> check{"x больше 0?"}',
  '  check -->|да| out["Вывод на экран"]',
  '  check -->|нет| err["Сообщение об ошибке"]',
);

// The document below comes from route.fulfill and so has no server address: Chromium's Local
// Network Access checks would then block Vite's HMR WebSocket to localhost (a dev-server detail;
// the real window loads the bundle from disk). Worker-scoped, so it is set for this file only.
test.use({
  launchOptions: {
    ...(process.env.CW_E2E_CHROMIUM ? { executablePath: process.env.CW_E2E_CHROMIUM } : {}),
    args: ["--disable-features=LocalNetworkAccessChecks,LocalNetworkAccessChecksWebSockets"],
  },
});

/**
 * Serves the app under the window CSP. Vite's dev index.html carries inline module scripts
 * (React Refresh preamble); like Tauri does for bundled pages, only their sha256 hashes are
 * added to script-src. Nothing else is relaxed: no 'unsafe-eval', no extra hosts.
 * (Fetch.continueResponse with an added header is not enough: Chromium ignores a CSP header
 * added at the DevTools response stage, so the document is fulfilled.)
 */
async function enforceAppCsp(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as any;
    w.__cspViolations = [];
    document.addEventListener("securitypolicyviolation", (e) => {
      w.__cspViolations.push(`${e.effectiveDirective} blocked ${e.blockedURI || "inline"}: ${e.sample}`);
    });
  });
  await page.route(
    (url) => url.pathname === "/" || url.pathname === "/index.html",
    async (route) => {
      if (route.request().resourceType() !== "document") return route.fallback();
      const res = await route.fetch();
      const html = await res.text();
      const hashes = [...html.matchAll(/<script\b(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)].map(
        (m) => `'sha256-${crypto.createHash("sha256").update(m[1], "utf8").digest("base64")}'`,
      );
      const csp = APP_CSP.replace(/script-src([^;]*)/, (_all, v: string) => `script-src${v} ${hashes.join(" ")}`);
      const headers: Record<string, string> = { ...res.headers(), "content-security-policy": csp };
      delete headers["content-length"];
      await route.fulfill({ status: res.status(), headers, body: html });
    },
  );
}

async function cspViolations(page: Page): Promise<string[]> {
  return page.evaluate(() => [...((window as any).__cspViolations ?? [])]);
}

/** No console errors, no uncaught exceptions and no CSP violations so far. */
async function expectCleanConsole(app: AppDriver): Promise<void> {
  expect(await cspViolations(app.page), "CSP violations").toEqual([]);
  expect(app.consoleErrors, "console errors").toEqual([]);
  expect(app.pageErrors, "page errors").toEqual([]);
}

/** Each source line is on its own row (not laid out side by side). */
async function expectLinesStacked(lines: Locator, count: number): Promise<void> {
  await expect(lines).toHaveCount(count);
  const ys = await lines.evaluateAll((els) => els.map((e) => e.getBoundingClientRect().y));
  for (let i = 1; i < ys.length; i++) expect(ys[i]).toBeGreaterThan(ys[i - 1]);
}

/** Seeds a finished thread with one answer and opens it from the sidebar. */
async function openAnswer(app: AppDriver, title: string, answer: string): Promise<void> {
  await app.backend.seedThread({
    title,
    turns: [
      {
        prompt: "Объясни на схеме",
        script: async (t) => {
          await t.start();
          await t.message(answer);
          await t.complete();
        },
      },
    ],
  });
  await app.boot();
  await app.ready();
  await app.page.locator("aside").first().getByText(title).click();
  await expect(app.main.getByText("Объясни на схеме").last()).toBeVisible();
}

test.describe("Схемы mermaid в ответах", () => {
  test.beforeEach(async ({ page }) => {
    await enforceAppCsp(page);
  });

  test("flowchart LR рисуется в SVG с подписями узлов на кириллице, панель схемы работает", async ({ app, page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await openAnswer(app, "Поток данных", `Вот как значение проходит через переменную:\n\n${FLOW}\n\nТак работает присваивание.`);

    const block = app.main.locator(".mermaid-block");
    await expect(block).toHaveCount(1, { timeout: 15_000 });
    const svg = block.locator(".mermaid-view > svg");
    await expect(svg).toBeVisible();
    await expect(svg).toHaveAttribute("id", /^cw-mermaid-\d+$/);
    await expect(svg).toHaveAttribute("aria-roledescription", "flowchart-v2");
    await expect(svg.locator("g.node")).toHaveCount(5);
    for (const label of ["Ввод: x = 5", "Переменная x", "x больше 0?", "Вывод на экран", "Сообщение об ошибке", "да", "нет"]) {
      await expect(svg.getByText(label, { exact: true })).toBeVisible();
    }
    // The text around the diagram is untouched, and no plain code block is left for it.
    await expect(app.main.locator(".md p", { hasText: "Так работает присваивание." })).toBeVisible();
    await expect(app.main.locator(".code-block:not(.mermaid-block)")).toHaveCount(0);
    await expect(block).toHaveAttribute("data-code", /^flowchart LR/);

    // LR: the input node is left of the output node; the SVG fits the message width.
    const inBox = (await svg.getByText("Ввод: x = 5", { exact: true }).boundingBox())!;
    const outBox = (await svg.getByText("Вывод на экран", { exact: true }).boundingBox())!;
    expect(inBox.x + inBox.width).toBeLessThan(outBox.x);
    const svgBox = (await svg.boundingBox())!;
    const blockBox = (await block.boundingBox())!;
    expect(svgBox.width).toBeLessThanOrEqual(blockBox.width);
    // Labels use the UI font.
    expect(await svg.evaluate((el) => getComputedStyle(el).fontFamily)).toContain("Segoe UI");

    // "Код" toggles the source view.
    const codeBtn = block.getByRole("button", { name: "Код", exact: true });
    await codeBtn.click();
    await expect(codeBtn).toHaveAttribute("aria-pressed", "true");
    await expect(block.locator(".mermaid-view")).toHaveCount(0);
    await expect(block.locator("pre")).toContainText('store --> check{"x больше 0?"}');
    await expectLinesStacked(block.locator("pre .line"), 5);
    await codeBtn.click();
    await expect(codeBtn).toHaveAttribute("aria-pressed", "false");
    await expect(svg).toBeVisible();

    // "Копировать SVG" puts the SVG markup on the clipboard.
    await block.getByRole("button", { name: "Копировать SVG" }).click();
    await expect(block.getByText("Скопировано")).toBeVisible();
    const clip = await page.evaluate(() => navigator.clipboard.readText());
    expect(clip.startsWith("<svg")).toBe(true);
    expect(clip).toContain("Переменная x");

    // Full-size view in an overlay with its own copy of the SVG; Esc closes it.
    const id = await svg.getAttribute("id");
    await block.getByRole("button", { name: "Во весь размер" }).click();
    const overlay = page.getByRole("dialog", { name: "Схема во весь размер" });
    await expect(overlay).toBeVisible();
    const big = overlay.locator('svg[id^="cw-mermaid"]');
    await expect(big).toHaveAttribute("id", `${id}-full`);
    await expect(big.getByText("Переменная x", { exact: true })).toBeVisible();
    expect((await big.boundingBox())!.width).toBeGreaterThanOrEqual(svgBox.width - 1);
    await overlay.getByRole("button", { name: "Увеличить" }).click();
    await expect(overlay.getByRole("button", { name: /^\d+%$/ })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(overlay).toHaveCount(0);

    // The diagram follows the app theme: switching to light re-renders it with light colors.
    const fill = () => svg.evaluate((el) => getComputedStyle(el.querySelector("g.node rect, g.node path")!).fill);
    const darkFill = await fill();
    await page.evaluate(() => (document.documentElement.dataset.theme = "light"));
    await expect(svg).not.toHaveAttribute("id", id!);
    await expect.poll(fill).not.toBe(darkFill);
    await page.evaluate(() => (document.documentElement.dataset.theme = "dark"));
    await expect.poll(fill).toBe(darkFill);

    await expectCleanConsole(app);
  });

  test("неверная схема: строка об ошибке и исходный код вместо пустого места", async ({ app, page }) => {
    const broken = fence("flowchart LR", '  A["Начало"] --> B["Конец"');
    const unknown = fence("схема данных", "  А --> Б");
    const valid = fence("flowchart TD", '  a["Сначала"] --> b["Потом"]');
    await openAnswer(app, "Сломанная схема", `Первая:\n\n${broken}\n\nВторая:\n\n${unknown}\n\nТретья:\n\n${valid}`);

    const errors = app.main.locator(".mermaid-error");
    await expect(errors).toHaveCount(2, { timeout: 15_000 });
    await expect(errors.nth(0).getByText(/^Схема не распознана: .+/)).toBeVisible();
    await expect(errors.nth(1).getByText(/^Схема не распознана: .+/)).toBeVisible();
    // The source stays readable as an ordinary code block.
    await expect(errors.nth(0).locator(".code-block")).toHaveAttribute("data-code", /A\["Начало"\] --> B\["Конец"/);
    await expect(errors.nth(0).locator(".code-block")).toContainText('B["Конец"');
    await expectLinesStacked(errors.nth(0).locator(".code-block .line"), 2);
    await expect(errors.nth(1).locator(".code-block")).toContainText("схема данных");
    // Full parser message on demand.
    await errors.nth(0).getByText("Подробнее").click();
    await expect(errors.nth(0).locator("details pre")).toBeVisible();

    // A valid diagram after the broken ones still renders.
    await expect(app.main.locator(".mermaid-block .mermaid-view > svg")).toHaveCount(1);
    await expect(app.main.locator(".mermaid-block").getByText("Потом", { exact: true })).toBeVisible();

    // mermaid did not leave its own error picture or temporary containers in the page.
    await expect(page.locator('svg[aria-roledescription="error"]')).toHaveCount(0);
    await expect(page.locator('body > [id^="dcw-mermaid"], body > [id^="icw-mermaid"]')).toHaveCount(0);
    await expect(page.locator('svg[id^="cw-mermaid"]')).toHaveCount(1);

    await expectCleanConsole(app);
  });

  test("пока ответ идёт потоком, схема показана как код; после завершения рисуется", async ({ app, backend }) => {
    await app.boot();
    await app.ready();
    const head = `Схема:\n\n${FLOW}\n`;
    const answer = `${head}\nПеременная хранит значение, а стрелки показывают, куда оно идёт дальше.`;
    backend.scriptNextTurn(async (t) => {
      await t.start();
      // The first delta carries the whole fenced block; the turn then waits on the gate.
      await t.message(answer, { chunk: head.length, delayMs: 5, pauseAfterFirstDelta: "diagram-streamed" });
      await t.complete();
    });
    await app.newSession();
    await app.send("Нарисуй поток данных");

    const source = app.main.locator('.code-block[data-code^="flowchart LR"]');
    await expect(source).toBeVisible();
    await expect(source.getByText("mermaid", { exact: true })).toBeVisible();
    await expect(app.main.locator(".md.stream-cursor")).toHaveCount(1);
    await expect(app.main.locator(".mermaid-block")).toHaveCount(0);

    backend.release("diagram-streamed");
    await expect(app.main.locator(".mermaid-block .mermaid-view > svg")).toBeVisible({ timeout: 15_000 });
    await expect(app.main.locator(".mermaid-block").getByText("Переменная x", { exact: true })).toBeVisible();
    await expect(app.main.locator(".md").last()).toContainText("куда оно идёт дальше");

    await expectCleanConsole(app);
  });

  test("под CSP окна Tauri разные типы схем рисуются без нарушений CSP и ошибок в консоли", async ({ app, page }) => {
    const diagrams = [
      FLOW,
      fence("sequenceDiagram", "  participant П as Программа", "  participant П2 as Память", "  П->>П2: записать x = 5", "  П2-->>П: готово"),
      fence("classDiagram", "  class Вектор {", "    +float x", "    +float y", "    +длина() float", "  }"),
      fence("stateDiagram-v2", "  [*] --> Пусто", "  Пусто --> Заполнено: присвоить", "  Заполнено --> [*]"),
      fence("pie title Типы данных", '  "Числа" : 40', '  "Строки" : 35', '  "Списки" : 25'),
      fence("mindmap", "  root((Переменная))", "    Имя", "    Значение", "    Тип"),
      fence("erDiagram", "  ПОЛЬЗОВАТЕЛЬ ||--o{ ЗАКАЗ : делает"),
    ];
    await openAnswer(app, "Разные схемы", diagrams.map((d, i) => `Схема ${i + 1}:\n\n${d}`).join("\n\n"));

    const svgs = app.main.locator(".mermaid-block .mermaid-view > svg");
    await expect(svgs).toHaveCount(diagrams.length, { timeout: 20_000 });
    await expect(app.main.locator(".mermaid-error")).toHaveCount(0);
    await expect(svgs.nth(1).getByText("записать x = 5")).toBeAttached();
    await expect(svgs.nth(4).getByText("Строки")).toBeAttached();
    await expect(svgs.nth(5).getByText("Значение", { exact: true })).toBeAttached();

    await expectCleanConsole(app);

    // The policy is really enforced: eval is refused and an inline script is blocked and reported.
    // Both probes run from a timer, outside page.evaluate (CDP evaluation itself may eval).
    await page.evaluate(() => {
      const w = window as any;
      setTimeout(() => {
        try {
          w.__evalProbe = String(new Function("return 'eval allowed'")());
        } catch (e) {
          w.__evalProbe = (e as Error).name;
        }
        const s = document.createElement("script");
        s.textContent = "window.__inlineRan = true";
        document.head.appendChild(s);
      }, 0);
    });
    await expect.poll(() => page.evaluate(() => (window as any).__evalProbe)).toBe("EvalError");
    await expect
      .poll(() => cspViolations(page))
      .toEqual([expect.stringMatching(/^script-src blocked eval/), expect.stringMatching(/^script-src-elem blocked inline/)]);
    expect(await page.evaluate(() => (window as any).__inlineRan ?? false)).toBe(false);
  });
});
