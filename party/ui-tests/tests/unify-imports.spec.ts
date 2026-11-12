import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, measureOverflow, minTap, scrollIntoReach, signedInShell, test } from "./fixtures";

/**
 * One Import screen (0.1.5): every place that starts an import leads to it (ui/party/entry/unifyImports.ts). A file
 * pick opens the screen with its file chooser, and the chosen file shows up in the screen's results; a link form takes
 * its address there; files dropped on the window go there too. Run on the three phones and a desktop window.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const hero = () => ({ name: "Unify Hero.dnd5e", mimeType: "application/octet-stream", buffer: readFileSync(join(FIXTURES, "d5-hero.dnd5e")) });

const desktop = (testInfo: TestInfo) => testInfo.project.name === "desktop";

/** Opens a route: the phone layer on a phone (`?mock=mobile`), a plain browser window on the desktop. */
async function go(page: Page, testInfo: TestInfo, route: string): Promise<void> {
  if (desktop(testInfo)) await page.goto(`/#${route}`);
  else {
    await page.goto(`/?mock=mobile#${route}`);
    await expect(page.locator("html.phone")).toHaveCount(1);
  }
  await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
  await signedInShell(page);
}

const results = (page: Page) => page.getByRole("list", { name: "Import results" });
const row = (page: Page, title: string) => results(page).locator("li").filter({ has: page.locator(".party-import-name", { hasText: title }) });

async function onImportScreen(page: Page, testInfo: TestInfo): Promise<void> {
  await expect(page).toHaveURL(/#\/import-files/);
  await expect(page.getByRole("button", { name: "Choose files" })).toBeVisible();
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  // The Import screen's own controls are big enough to tap on a phone.
  if (!desktop(testInfo)) {
    const small = await page.evaluate((min) => Array.from(document.querySelectorAll<HTMLElement>(".party-import button, .party-import input:not([type=file])"))
      .filter((el) => el.offsetParent !== null && !(el as HTMLButtonElement).disabled)
      .map((el) => ({ label: el.textContent?.trim() || el.getAttribute("aria-label") || "", r: el.getBoundingClientRect() }))
      .filter(({ r }) => r.height < min - 0.5)
      .map(({ label, r }) => `${label} ${Math.round(r.width)}x${Math.round(r.height)}`), minTap(testInfo));
    expect(small, `Import screen controls under ${minTap(testInfo)} px`).toEqual([]);
  }
}

/** Taps (phones) or clicks (desktop) `target`; it must open the file chooser and land on the Import screen. */
async function pickThrough(page: Page, testInfo: TestInfo, target: Locator): Promise<void> {
  expect(await scrollIntoReach(page, target, page.viewportSize()!.height), "the entry point can be reached").toBe(true);
  const chooser = page.waitForEvent("filechooser");
  if (desktop(testInfo)) await target.click();
  else await target.tap();
  const picked = await chooser;
  await onImportScreen(page, testInfo);
  await picked.setFiles([hero()]);
  await expect(row(page, "Unify Hero.dnd5e").getByRole("status")).toHaveText(/^Imported /, { timeout: 30_000 });
}

/** Opens upstream's File menu: the desktop menu bar, or the phone's Menu sheet. */
async function fileMenu(page: Page, testInfo: TestInfo): Promise<Locator> {
  if (desktop(testInfo) && await page.locator(".aur-menubar").isVisible()) {
    await page.getByRole("menuitem", { name: "File", exact: true }).click();
    return page.getByRole("menu");
  }
  await page.getByRole("button", { name: "Menu" }).tap();
  return page.getByRole("dialog", { name: "Menu" });
}

const menuItem = (menu: Locator, name: string) => menu.locator(".aur-menu-item, .menu-tree-item").filter({ hasText: new RegExp(`^${name.replace(/[.]/g, "\\.")}$`) });

test.describe("every import entry point leads to the Import screen", () => {
  test.beforeEach(async ({ signIn }) => {
    await signIn("player");
  });

  test("Start: Import characters", async ({ page }, testInfo) => {
    await go(page, testInfo, "/characters");
    await pickThrough(page, testInfo, page.getByRole("button", { name: /Import characters/ }));
  });

  // "Import Characters" is shown only with upstream's AI import, which the party has none of (vitest covers it).
  for (const item of ["Load Character", "Open bundle..."]) {
    test(`File menu: ${item}`, async ({ page }, testInfo) => {
      await go(page, testInfo, "/characters");
      const menu = await fileMenu(page, testInfo);
      await pickThrough(page, testInfo, menuItem(menu, item));
      await expect(menu, "the menu closed").toBeHidden();
    });
  }

  for (const name of ["Import files", "Choose a .zip of a 5etools-format data/ folder", "Choose 5etools-format homebrew .json files"]) {
    test(`Additional Content: ${name}`, async ({ page }, testInfo) => {
      await go(page, testInfo, "/content");
      await pickThrough(page, testInfo, page.getByRole("button", { name, exact: true }));
    });
  }

  for (const form of [
    { name: "Index files: Fetch", field: "Index URL", button: "Fetch", url: "https://example.invalid/unify.index" },
    { name: "5etools: Import from an address", field: "Address of 5etools-format data", button: "Import", url: "https://example.invalid/unify-data/" },
  ]) {
    test(`Additional Content: ${form.name}`, async ({ page }, testInfo) => {
      await go(page, testInfo, "/content");
      const field = page.getByLabel(form.field);
      expect(await scrollIntoReach(page, field, page.viewportSize()!.height)).toBe(true);
      await field.fill(form.url);
      const button = field.locator("xpath=ancestor::form").getByRole("button", { name: form.button, exact: true });
      if (desktop(testInfo)) await button.click();
      else await button.tap();
      await onImportScreen(page, testInfo);
      // The address arrived and is being taken: the content notice or the first-download question comes first.
      await expect(row(page, form.url)).toBeVisible();
      const ask = page.getByRole("dialog", { name: /Add content source|Import only content you own/ });
      await expect(ask).toBeVisible({ timeout: 30_000 });
    });
  }

  test("files dropped on the window", async ({ page }, testInfo) => {
    await go(page, testInfo, "/characters");
    const bytes = [...hero().buffer];
    await page.evaluate((bytes) => {
      const data = new DataTransfer();
      data.items.add(new File([new Uint8Array(bytes)], "Unify Hero.dnd5e"));
      const target = document.querySelector("main")!;
      target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: data }));
      target.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: data }));
    }, bytes);
    await onImportScreen(page, testInfo);
    await expect(row(page, "Unify Hero.dnd5e").getByRole("status")).toHaveText(/^Imported /, { timeout: 30_000 });
  });

  test("the main doors stay: More > Import on a phone, Import in the desktop account bar", async ({ page }, testInfo) => {
    if (desktop(testInfo)) {
      await go(page, testInfo, "/characters");
      const button = page.locator(".party-bar").getByRole("button", { name: "Import", exact: true });
      await button.click();
    } else {
      await go(page, testInfo, "/more");
      const link = page.getByRole("list", { name: "Account" }).getByRole("link", { name: "Import", exact: true });
      expect(await scrollIntoReach(page, link, page.viewportSize()!.height)).toBe(true);
      expect((await link.boundingBox())!.height).toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
      await link.tap();
    }
    await onImportScreen(page, testInfo);
  });
});
