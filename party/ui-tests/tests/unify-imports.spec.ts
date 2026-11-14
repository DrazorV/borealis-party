import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, measureOverflow, minTap, scrollIntoReach, signedInShell, test } from "./fixtures";

/**
 * One import page (0.2.1): the Import section at the top of Additional Content, and no other. Every place that starts
 * an import leads there (ui/party/entry/unifyImports.ts, ui/party/import/door.ts): a file pick opens the section's file
 * chooser and the chosen file shows up in its results; files dropped on the window go there too. Upstream's own import
 * controls on the page are hidden, and the old Import screen address leads to the page. Run on the three phones and a
 * desktop window.
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

/** On Additional Content, with its Import section in view. */
async function onImportScreen(page: Page, testInfo: TestInfo): Promise<void> {
  await expect(page).toHaveURL(/#\/content$/);
  const section = page.locator("section#party-import");
  await expect(section).toHaveCount(1);
  await expect(section.getByRole("button", { name: "Choose files" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Choose files" }), "one file chooser button").toHaveCount(1);
  const top = (await section.boundingBox())!.y;
  expect(top, "the Import section is in view").toBeLessThan(page.viewportSize()!.height);
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

test.describe("every import entry point leads to the Import section on Additional Content", () => {
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

  test("Additional Content is the one import page: upstream's own import controls are hidden", async ({ page }, testInfo) => {
    await go(page, testInfo, "/content");
    await onImportScreen(page, testInfo);
    // The section comes first, before the installed packs.
    const order = await page.evaluate(() => {
      const all = Array.from(document.querySelectorAll<HTMLElement>(".v4-content section.v4-csec")).filter((s) => s.offsetParent !== null);
      return all.map((s) => s.getAttribute("aria-label"));
    });
    expect(order[0]).toBe("Import");
    expect(order).toContain("Installed packs");
    for (const name of ["Import files", "Choose a .zip of a 5etools-format data/ folder", "Choose 5etools-format homebrew .json files", "Fetch"]) {
      await expect(page.getByRole("button", { name, exact: true }), `${name} is not offered twice`).toBeHidden();
    }
    await expect(page.getByLabel("Index URL")).toBeHidden();
    await expect(page.getByLabel("Address of 5etools-format data")).toBeHidden();
    // Management stays: the installed packs, and one Update all content (0.3.3) in place of upstream's Update content files.
    await expect(page.getByRole("region", { name: "Installed packs" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Update content" })).toBeVisible();
    await expect(page.getByRole("region", { name: "Update content files" })).toBeHidden();
    await expect(page.getByRole("region", { name: "Import from an address" })).toBeHidden();
    await page.screenshot({ path: testInfo.outputPath("one-import-page.png") });
    await page.locator("section.v4-csec[aria-label='Installed packs']").scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath("one-import-page-management.png") });
  });

  test("the old Import screen address leads to Additional Content", async ({ page }, testInfo) => {
    await go(page, testInfo, "/import-files");
    await onImportScreen(page, testInfo);
  });

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
