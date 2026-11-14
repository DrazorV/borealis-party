import { join } from "node:path";
import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, open, signedInShell, test } from "./fixtures";

/**
 * The DM tracker on the S26 Ultra (384x832): the sticky resource panel is one full-width line while collapsed and
 * never covers the turn panel (0.3.2 follow-up). Against the tracker's mock server (`party.combatMock`).
 */

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-combat-phone-${name}.png`), fullPage: false });
}

async function turnOf(page: Page, who: string) {
  await page.addInitScript(() => { localStorage.setItem("party.combatMock", "1"); localStorage.removeItem("party.combatMock.state"); });
  await open(page, "/combat/c1");
  await signedInShell(page);
  await expect(page.locator("[data-combatant]").first()).toBeVisible({ timeout: 30_000 });
  for (const [label, n] of [["Knight", 18], ["Adult Red Dragon", 12], ["Priest", 10], ["Cultist 1", 9], ["Cultist 2", 8], ["Mage", 5], ["Druid", 4]] as const) {
    const box = page.getByLabel(`${label} initiative`, { exact: true });
    await box.fill(String(n));
    await box.press("Enter");
  }
  const next = page.locator(".cb-thumb:visible .btn").first();
  await next.click();
  for (let i = 0; i < 12 && !(await page.getByRole("heading", { name: `${who}'s turn` }).isVisible()); i++) await next.click();
  await expect(page.getByRole("heading", { name: `${who}'s turn` })).toBeVisible();
  return next;
}

const box = async (l: Locator) => (await l.boundingBox())!;

/** Scrolls `l` to the top of its scroller (its scroll-margin applies) and waits for it to settle. */
async function scrollToTop(page: Page, l: Locator) {
  await l.evaluate((el) => el.scrollIntoView({ block: "start", behavior: "instant" }));
  await page.waitForTimeout(150);
}

test.describe("The tracker's sticky resource panel on a phone (0.3.2)", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "the S26 Ultra's 384x832");
  });

  test("collapsed it is one full-width line, and nothing of the turn panel scrolls under it", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    const next = await turnOf(page, "Mage");
    const resources = page.locator(".cb-right");
    const panel = page.locator("[data-turn-panel]");
    await expect(panel).toBeVisible();

    const r = await box(resources);
    const vw = page.viewportSize()!.width;
    expect(r.width, "the panel spans the page").toBeGreaterThanOrEqual(vw - 2 * 16);
    expect(r.height, "collapsed, the panel is one line").toBeLessThanOrEqual(72);
    await expect(page.locator(".cb-res-summary")).toContainText("Adult Red Dragon 3/3");

    // Every group of the turn panel, scrolled to the top, starts below the panel.
    const targets = [panel, page.getByRole("toolbar", { name: "Reactions ready" }), ...(await panel.locator(".cb-tb-group").all())];
    for (const t of targets) {
      await scrollToTop(page, t);
      const res = await box(resources);
      const b = await box(t);
      expect(b.y, `${await t.getAttribute("aria-label") ?? "the turn panel"} starts below the resource panel`).toBeGreaterThanOrEqual(res.y + res.height - 1);
    }
    await shot(page, testInfo, "groups");

    // The sticky reactions strip, scrolled past, sits under the panel, not behind it.
    await scrollToTop(page, panel.locator(".cb-tb-group").last());
    const strip = page.getByRole("toolbar", { name: "Reactions ready" });
    if (await strip.isVisible()) {
      const s = await box(strip);
      const res = await box(resources);
      expect(s.y, "the reactions strip sticks below the resource panel").toBeGreaterThanOrEqual(res.y + res.height - 1);
    }
    await shot(page, testInfo, "strip");

    // Next turn brings the new card below the panel.
    await next.click();
    await expect(page.getByRole("heading", { name: "Druid's turn" })).toBeVisible();
    await expect.poll(async () => {
      const res = await box(resources);
      const card = await box(page.locator("#cb-card"));
      return card.y >= res.y + res.height - 1;
    }, { message: "the new turn's card starts below the resource panel" }).toBe(true);

    // More opens the legendary options; Less folds it back to one line.
    await page.locator(".cb-resources .cb-more-toggle").click();
    await expect(page.getByRole("region", { name: "Adult Red Dragon legendary actions" })).toBeVisible();
    await shot(page, testInfo, "expanded");
    await page.locator(".cb-resources .cb-more-toggle").click();
    await expect(page.getByRole("region", { name: "Adult Red Dragon legendary actions" })).toBeHidden();
    expect((await box(resources)).height).toBeLessThanOrEqual(72);
  });
});
