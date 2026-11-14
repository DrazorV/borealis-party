import type { Page, Route, TestInfo } from "@playwright/test";
import { join } from "node:path";
import { packs, SOURCE_URL, sources } from "./contentFixture";
import { expect, measureOverflow, minTap, open, signedInShell, smallTargets, test } from "./fixtures";

/**
 * Additional Content, "Update content" (0.3.3) at 1440x900 (desktop-wide) and 384x832 (android-portrait):
 * - one "Update all content" button for the index files and every 5etools source; upstream's "Update content files"
 *   and its "Import from an address" list are gone, so the 5etools source shows once (in the 5etools card, with
 *   Update and Remove);
 * - the last check, whether the account's Automatic Updates is on, and the status bar's real last-check time;
 * - the button runs the real job on the rig server (the account has nothing to update, so it says so).
 * The 5etools source and the facts on the line are answered in the browser; the job is the server's.
 */

async function stub(page: Page) {
  const json = (route: Route, body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(/\/api\/rpc\/list_packs$/, (route) => json(route, packs()));
  await page.route(/\/api\/rpc\/list_5etools_urls$/, (route) => json(route, sources()));
  await page.route(/\/api\/rpc\/content_update_info$/, (route) => json(route, {
    auto: true, everyHours: 24, indexes: 2, sources: 1,
    last: { indexes: 2, sources: 1, updated: 1, failed: [], message: "2 index files and 1 5etools source checked · 1 updated", checkedAt: "2026-11-14T08:00:00Z", auto: true },
  }));
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-update-all-${name}.png`) });
}

test("Update content: one button, each 5etools source once, the automatic check (0.3.3)", async ({ page, signIn }, testInfo) => {
  test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "1440x900 and 384x832");
  await signIn("player");
  await stub(page);
  if (testInfo.project.name === "android-portrait") await open(page, "/content");
  else { await page.goto("/#/content"); await signedInShell(page); }

  const card = page.getByRole("region", { name: "Update content" });
  await expect(card).toBeVisible({ timeout: 30_000 });
  await expect(card.getByRole("status")).toContainText("Last checked");
  await expect(card.getByRole("status")).toContainText("2 index files and 1 5etools source checked · 1 updated");
  await expect(card).toContainText("Automatic Updates is on");
  await expect(card.getByRole("checkbox")).toHaveCount(0);
  // One Update all; upstream's own update section and address list are gone.
  await expect(page.getByRole("button", { name: "Update all content" })).toHaveCount(1);
  await expect(page.getByRole("region", { name: "Update content files" })).toBeHidden();
  await expect(page.getByRole("region", { name: "Import from an address" })).toBeHidden();
  // The 5etools source is listed once, in the 5etools card, with its own Update and Remove.
  await expect(page.getByText(SOURCE_URL, { exact: true }).filter({ visible: true })).toHaveCount(1);
  await expect(page.getByRole("button", { name: `Update from ${SOURCE_URL}` })).toBeVisible();
  await expect(page.getByRole("button", { name: `Remove ${SOURCE_URL}` })).toBeVisible();

  await card.scrollIntoViewIfNeeded();
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  if (testInfo.project.name === "android-portrait") {
    const small = (await smallTargets(page, minTap(testInfo), "section[aria-label='Update content']")).map((t) => `${t.label} ${t.width}x${t.height}`);
    expect(small, "small controls in Update content").toEqual([]);
  }
  await shot(page, testInfo, "card");

  // The real job: this account has no index file or source on the server, so it says so.
  await card.getByRole("button", { name: "Update all content" }).click();
  await expect(card.getByRole("status")).toContainText("Nothing to update", { timeout: 30_000 });
  // The status bar now says when this account's content was really checked (the job just wrote it), not the page load.
  await expect(page.locator(".status-msg").filter({ visible: true }).first()).toHaveText(/^Last checked for content updates at \d\d:\d\d$/, { timeout: 15_000 });
  await shot(page, testInfo, "ran");
});
