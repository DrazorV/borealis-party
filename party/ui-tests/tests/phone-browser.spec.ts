import type { Page } from "@playwright/test";
import { expect, minTap, settled, signedInShell, test } from "./fixtures";

/**
 * Plan M4 (D8): a plain phone browser, without the web mock (`?mock=mobile`), gets the apps' phone layer: the platform
 * classes, the keyboard inset, a browser Back that closes the top sheet or dialog, and a visible Close on the Menu sheet.
 * Android Chrome is Chromium with an Android user agent and touch; iOS Safari is WebKit as an iPhone.
 */

/** Open a route the way a phone browser would: no `?mock=mobile`. */
async function openPlain(page: Page, route: string): Promise<void> {
  await page.goto(`/#${route}`);
  await signedInShell(page);
}

const hash = (page: Page) => page.evaluate(() => window.location.hash);
const back = (page: Page) => page.evaluate(() => history.back());

async function openMenu(page: Page) {
  await page.getByRole("button", { name: "Menu" }).tap();
  const menu = page.getByRole("dialog", { name: "Menu" });
  await settled(menu);
  return menu;
}

test.describe("phone browser", () => {
  test.beforeEach(async ({ signIn }, testInfo) => {
    test.skip(testInfo.project.name === "android-landscape", "the browser phone layer is checked in portrait");
    await signIn("admin");
  });

  test("gets the phone layer and its platform class without the web mock", async ({ page }, testInfo) => {
    await openPlain(page, "/characters");
    const os = testInfo.project.name.startsWith("android") ? "android" : "ios";
    await expect(page.locator(`html.phone.${os}`)).toHaveCount(1);
    expect(await page.evaluate(() => window.location.search)).toBe("");
  });

  test("the Menu sheet closes with a visible Close button", async ({ page }, testInfo) => {
    await openPlain(page, "/characters");
    const menu = await openMenu(page);
    const close = menu.getByRole("button", { name: "Close" });
    await expect(close).toBeVisible();
    const box = (await close.boundingBox())!;
    const min = minTap(testInfo);
    expect(box.width, "Close is wide enough to tap").toBeGreaterThanOrEqual(min);
    expect(box.height, "Close is tall enough to tap").toBeGreaterThanOrEqual(min);
    const { width } = page.viewportSize()!;
    expect(box.x + box.width, "Close is on the screen").toBeLessThanOrEqual(width);
    await close.tap();
    await expect(menu).toBeHidden();
    // Closed by its button, the sheet leaves no history entry behind: Back still leaves the page.
    expect(await hash(page)).toBe("#/characters");
  });

  test("Back closes the top sheet and stays on the page; the next Back leaves it", async ({ page }) => {
    await openPlain(page, "/characters");
    await page.getByRole("link", { name: "Sources" }).first().tap();
    await expect.poll(() => hash(page)).toBe("#/content/sources");
    const menu = await openMenu(page);
    await back(page);
    await expect(menu).toBeHidden();
    expect(await hash(page)).toBe("#/content/sources");
    await expect(page.locator("main")).toBeVisible();
    await back(page);
    await expect.poll(() => hash(page)).toBe("#/characters");
  });

  test("history isn't broken by a sheet closed with its button", async ({ page }) => {
    await openPlain(page, "/characters");
    await page.getByRole("link", { name: "Sources" }).first().tap();
    await expect.poll(() => hash(page)).toBe("#/content/sources");
    for (let i = 0; i < 2; i++) {
      const menu = await openMenu(page);
      await menu.getByRole("button", { name: "Close" }).tap();
      await expect(menu).toBeHidden();
    }
    // Let the sheet's entry come off (the page's own entry is current again), then one Back goes to the page before.
    await expect.poll(() => page.evaluate(() => (history.state as { partyOverlay?: number } | null)?.partyOverlay ?? null)).toBeNull();
    await back(page);
    await expect.poll(() => hash(page)).toBe("#/characters");
    // And Forward comes back to it.
    await page.evaluate(() => history.forward());
    await expect.poll(() => hash(page)).toBe("#/content/sources");
  });

  test("Back closes Application Settings, a dialog over the page", async ({ page }) => {
    await openPlain(page, "/characters");
    await page.getByRole("button", { name: "Settings" }).first().tap();
    const settings = page.getByRole("dialog").first();
    await settled(settings);
    await back(page);
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(await hash(page)).toBe("#/characters");
  });

  test("the keyboard inset applies", async ({ page }) => {
    // A browser keyboard shrinks the visual viewport, not the page: stand a visual viewport in for one we can shrink.
    await page.addInitScript(() => {
      const vv = new EventTarget() as EventTarget & { height: number; width: number; offsetTop: number; offsetLeft: number; scale: number };
      vv.height = window.innerHeight;
      vv.width = window.innerWidth;
      vv.offsetTop = 0;
      vv.offsetLeft = 0;
      vv.scale = 1;
      Object.defineProperty(window, "visualViewport", { configurable: true, get: () => vv });
      (window as unknown as { __kb: (px: number) => void }).__kb = (px) => {
        vv.height = window.innerHeight - px;
        vv.dispatchEvent(new Event("resize"));
      };
    });
    await openPlain(page, "/characters");
    const root = page.locator("html");
    await expect(root).toHaveCSS("--kb", "0px");
    await page.evaluate(() => (window as unknown as { __kb: (px: number) => void }).__kb(330));
    await expect(root).toHaveCSS("--kb", "330px");
    await expect(root).toHaveClass(/\bkb-open\b/);
    await page.evaluate(() => (window as unknown as { __kb: (px: number) => void }).__kb(0));
    await expect(root).toHaveCSS("--kb", "0px");
    await expect(root).not.toHaveClass(/\bkb-open\b/);
  });
});
