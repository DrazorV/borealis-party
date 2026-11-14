import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { expect, open, signedInShell, test } from "./fixtures";

/**
 * The Build page's content notice (ui/party/builder/NoticesBanner.tsx): the imported hero carries an item that is not
 * installed. The notice lines up with the selection rows below it (same left and right edge), keeps a gap before them,
 * reads as a sentence with the raw id under Details, and its × dismisses it until the page session ends.
 * Runs on the S26 Ultra's 384x832 (android-portrait) and at a 1440x900 desktop window (desktop-wide).
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
// the fixture's made-up homebrew item, which no installed content has
const MISSING_ID = "ID_SOME_HOMEBREW_THING";

async function api<T>(request: APIRequestContext, auth: string, path: string, data: unknown): Promise<T> {
  const r = await request.fetch(path, { method: "POST", headers: { authorization: `Bearer ${auth}` }, data });
  expect(r.ok(), `${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

const desktop = (testInfo: TestInfo) => !testInfo.project.use.isMobile;

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${desktop(testInfo) ? "desktop-1440" : testInfo.project.name}-notice-${name}.png`) });
}

test("the Build notice lines up with the rows, reads in words and can be dismissed", async ({ page, signIn, request }, testInfo) => {
  test.skip(!["android-portrait", "desktop-wide"].includes(testInfo.project.name), "384x832 and 1440x900 only");
  await signIn("player");
  const go = async (route: string) => {
    if (desktop(testInfo)) {
      await page.goto(`/#${route}`);
      await signedInShell(page);
    } else await open(page, route);
  };
  await go("/characters");
  const token = await page.evaluate(() => (JSON.parse(localStorage.getItem("party.session") ?? "{}") as { token?: string }).token ?? "");
  const { summary } = await api<{ summary: { name: string } }>(request, token, "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
  const name = summary.name;
  // open it from the collection, as a player does (the list was read before the import)
  await page.reload();
  await signedInShell(page);
  const card = page.getByRole("button", { name: new RegExp(`^${name},`) }).first();
  // a desktop opens a card on double-click; a phone selects it and shows Open
  if (desktop(testInfo)) await card.dblclick();
  else {
    await card.click();
    await page.getByRole("button", { name: `Open ${name}`, exact: true }).first().click();
  }
  await expect(page).not.toHaveURL(/#\/characters/, { timeout: 30_000 });
  await go("/builder/race");
  await expect(page.locator(".bp-left .pick-stack").first()).toBeVisible({ timeout: 30_000 });

  const notice = page.getByRole("region", { name: "Content notices" });
  await expect(notice).toBeVisible();
  // one notice is the sentence itself; several are a count that opens the list
  const toggle = notice.locator(".bp-notices-toggle");
  if (await toggle.count()) await toggle.click();
  await expect(notice).toContainText("isn't in your installed content");
  await expect(notice).not.toContainText(`inventory item ${MISSING_ID} is not installed`);
  const details = notice.locator("details").filter({ hasText: MISSING_ID });
  await details.locator("summary").click();
  await expect(details.locator("code")).toHaveText(MISSING_ID);
  await shot(page, testInfo, "open");

  const box = await notice.boundingBox();
  const row = await page.locator(".bp-left .pick-stack .aur-expander").first().boundingBox();
  expect(box && row, "the notice and the first selection group are on the page").toBeTruthy();
  expect(Math.abs(box!.x - row!.x), "same left edge as the rows").toBeLessThanOrEqual(1);
  expect(Math.abs(box!.x + box!.width - (row!.x + row!.width)), "same right edge as the rows").toBeLessThanOrEqual(1);
  expect(row!.y - (box!.y + box!.height), "a gap before the rows").toBeGreaterThanOrEqual(8);

  const close = notice.getByRole("button", { name: "Dismiss notices" });
  const c = await close.boundingBox();
  if (!desktop(testInfo)) {
    expect(Math.min(c!.width, c!.height), "the × is a 44 px touch target").toBeGreaterThanOrEqual(44);
    const d = await details.locator("summary").boundingBox();
    expect(d!.height, "Details is a 44 px touch target").toBeGreaterThanOrEqual(44);
  }
  await close.click();
  await expect(notice).toHaveCount(0);
  await shot(page, testInfo, "dismissed");
  // remembered for the session: another Build section and back
  await go("/builder/class");
  await go("/builder/race");
  await expect(page.locator(".bp-left .pick-stack").first()).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("region", { name: "Content notices" })).toHaveCount(0);
});
