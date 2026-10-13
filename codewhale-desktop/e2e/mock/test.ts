/**
 * Playwright fixtures: every test gets a fresh `MockBackend` attached to its page
 * and an `app` driver with the common UI steps.
 *
 *   import { test, expect } from "../mock/test";
 *   test("…", async ({ app, backend, page }) => { await app.boot(); … });
 */
import { test as base, expect, type Locator, type Page } from "@playwright/test";
import { MockBackend } from "./backend";

export { expect };

export class AppDriver {
  readonly pageErrors: string[] = [];
  readonly consoleErrors: string[] = [];
  /** Diagnostics of boots that needed a reload (Vite overlay from an in-flight edit). */
  readonly bootRecoveries: string[] = [];

  constructor(
    readonly page: Page,
    readonly backend: MockBackend,
  ) {
    page.on("pageerror", (e) => this.pageErrors.push(`${e.name}: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") this.consoleErrors.push(m.text());
    });
  }

  /**
   * Loads the app and waits until it registered its Tauri event listeners and loaded settings.
   *
   * If the app does not mount because Vite shows its error overlay (typically a source
   * file caught half-written by a concurrent edit), it reloads once; a second failure
   * throws with the overlay message so the report names the broken file.
   */
  async boot(path = "/"): Promise<void> {
    await this.page.goto(path);
    if (!(await this.waitMounted(12_000))) {
      const first = await this.mountDiagnostics();
      if (!first.overlay) throw new Error(`App did not mount (no Tauri listeners registered).\n${first.text}`);
      // Give HMR a moment to pick up the finished edit, then retry once.
      await this.page.waitForTimeout(1_500);
      await this.page.reload();
      if (!(await this.waitMounted(12_000))) {
        const second = await this.mountDiagnostics();
        throw new Error(`App did not mount after a reload.\n${second.text}`);
      }
      // Errors of the broken first load are not the app's fault; keep a note for the report.
      this.bootRecoveries.push(first.text);
      this.pageErrors.length = 0;
      this.consoleErrors.length = 0;
    }
    await expect(this.page.locator("header")).toBeVisible();
    await expect.poll(() => this.backend.calls("settings_get").length).toBeGreaterThan(0);
  }

  private async waitMounted(timeout: number): Promise<boolean> {
    try {
      await this.page.waitForFunction(
        () => {
          const m = (window as any).__CW_MOCK__;
          return !!m && m.listenerCount("cw-event") > 0 && m.listenerCount("runtime-status") > 0;
        },
        undefined,
        { timeout },
      );
      return true;
    } catch {
      return false;
    }
  }

  /** Vite error overlay text and recent console errors, for boot failures. */
  private async mountDiagnostics(): Promise<{ overlay: boolean; text: string }> {
    const overlay = await this.page
      .evaluate(() => {
        const el = document.querySelector("vite-error-overlay") as (Element & { shadowRoot: ShadowRoot | null }) | null;
        if (!el) return null;
        const root = el.shadowRoot;
        const msg = root?.querySelector(".message")?.textContent ?? "";
        const file = root?.querySelector(".file")?.textContent ?? "";
        return `${file}\n${msg}`.trim() || "vite-error-overlay (no message)";
      })
      .catch(() => null);
    const lines = [
      overlay ? `Vite error overlay:\n${overlay}` : "No Vite error overlay.",
      this.consoleErrors.length ? `Console errors:\n${this.consoleErrors.slice(-10).join("\n")}` : "No console errors.",
      this.pageErrors.length ? `Page errors:\n${this.pageErrors.join("\n")}` : "",
    ];
    return { overlay: !!overlay, text: lines.filter(Boolean).join("\n") };
  }

  /** Waits until bootstrap picked a model (settings.defaultModel set and shown in the toolbar). */
  async ready(): Promise<void> {
    await expect.poll(() => this.backend.settings.defaultModel, { timeout: 10_000 }).toBeTruthy();
    await expect(this.page.locator("header").getByText(this.backend.settings.defaultModel, { exact: true })).toBeVisible();
  }

  get main(): Locator {
    return this.page.locator("main");
  }

  /** User prompt bubbles in the chat (styled with the --user-bubble token). */
  get userBubbles(): Locator {
    return this.main.locator('[class*="user-bubble"]');
  }

  get composer(): Locator {
    return this.page.locator("main textarea").last();
  }

  /** Clicks "Новая сессия" on the welcome screen and waits for the thread stream to go live. */
  async newSession(): Promise<string> {
    const before = new Set(this.backend.threads.keys());
    await this.main.getByRole("button", { name: "Новая сессия" }).click();
    await expect.poll(() => [...this.backend.threads.keys()].find((k) => !before.has(k))).toBeTruthy();
    const id = [...this.backend.threads.keys()].find((k) => !before.has(k))!;
    await expect.poll(() => this.backend.threads.get(id)?.subscribed).toBe(true);
    return id;
  }

  /** Types a prompt into the composer and sends it with Enter. */
  async send(text: string): Promise<void> {
    await this.composer.click();
    await this.composer.fill(text);
    await this.composer.press("Enter");
  }

  /** Emits a Tauri event to the page. */
  emit(event: string, payload: unknown): Promise<void> {
    return this.backend.emit(event, payload);
  }
}

export const test = base.extend<{ backend: MockBackend; app: AppDriver }>({
  backend: async ({ page }, use, testInfo) => {
    const backend = new MockBackend();
    await backend.attach(page);
    testInfo.annotations.push({
      type: "fixtures",
      description: backend.fx.hasLive ? "live recordings + synthetic fallback" : "synthetic only",
    });
    await use(backend);
    // Let in-flight scripts finish so nothing emits into a closed page.
    backend.releaseAll();
    await backend.settle(500);
    const unhandled = [...new Set(backend.unhandled)];
    if (unhandled.length) testInfo.annotations.push({ type: "unhandled", description: unhandled.join("; ") });
    if (backend.scriptErrors.length) {
      testInfo.annotations.push({ type: "script-errors", description: backend.scriptErrors.map(String).join("; ") });
    }
  },
  app: async ({ page, backend }, use, testInfo) => {
    const app = new AppDriver(page, backend);
    await use(app);
    for (const note of app.bootRecoveries) testInfo.annotations.push({ type: "boot-reloaded", description: note });
    if (app.pageErrors.length) {
      testInfo.annotations.push({ type: "page-errors", description: app.pageErrors.join("; ") });
      if (!process.env.CW_E2E_ALLOW_PAGE_ERRORS) {
        throw new Error(`Uncaught errors in the page:\n${app.pageErrors.join("\n")}`);
      }
    }
  },
});
