import { join } from "node:path";
import type { Page, TestInfo } from "@playwright/test";
import { expect, open, signedInShell, test } from "./fixtures";

/**
 * Ending a combat (0.3.2): End combat in the tracker's header (the More menu on a phone), the "All enemies are down"
 * hint that never ends the fight by itself, and the way back to the encounter. On a DM's 1440x900 screen and the S26
 * Ultra's 384x832, against the tracker's mock server (`party.combatMock`).
 */

const phone = (testInfo: TestInfo) => testInfo.project.name === "android-portrait";

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-combat-end-${name}.png`), fullPage: false });
}

async function started(page: Page, testInfo: TestInfo) {
  await page.addInitScript(() => { localStorage.setItem("party.combatMock", "1"); localStorage.removeItem("party.combatMock.state"); });
  if (phone(testInfo)) await open(page, "/combat/c1");
  else {
    await page.goto("/#/combat/c1");
    await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
  }
  await signedInShell(page);
  await expect(page.locator("[data-combatant]").first()).toBeVisible({ timeout: 30_000 });
  await page.getByRole("button", { name: "Roll initiatives" }).or(page.getByRole("button", { name: "⋯ More" })).first().waitFor();
  for (const [label, n] of [["Knight", 18], ["Adult Red Dragon", 12], ["Priest", 10], ["Cultist 1", 9], ["Cultist 2", 8], ["Mage", 5], ["Druid", 4]] as const) {
    const box = page.getByLabel(`${label} initiative`, { exact: true });
    await box.fill(String(n));
    await box.press("Enter");
  }
  await page.locator(".cb-thumb:visible .btn, .cb-top-actions .cb-next:visible").first().click();
  await expect(page.getByText("ROUND 1").first()).toBeVisible();
}

/** The header's End combat: a button on a desktop, an item of More on a phone. */
async function endFromHeader(page: Page, testInfo: TestInfo) {
  const header = page.locator(".cb-top");
  if (phone(testInfo)) {
    await expect(header.getByRole("button", { name: "End combat" })).toHaveCount(0);
    await header.getByRole("button", { name: "⋯ More" }).click();
    await shot(page, testInfo, "menu");
    await page.getByRole("menu", { name: "More combat actions" }).getByRole("menuitem", { name: "End combat" }).click();
  } else {
    await shot(page, testInfo, "header");
    await header.getByRole("button", { name: "End combat" }).click();
  }
}

test.describe("Ending a combat (0.3.2)", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(!["android-portrait", "desktop-wide"].includes(testInfo.project.name), "the S26 Ultra and a DM's 1440x900 screen");
  });

  test("End combat sits in the header, asks first, and the ended note leads back to the encounter", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await started(page, testInfo);
    await endFromHeader(page, testInfo);
    const dialog = page.getByRole("dialog", { name: "End this combat?" });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "End combat" }).click();
    const note = page.getByRole("status", { name: "Combat ended" });
    await expect(note).toBeVisible();
    await expect(page.getByRole("heading", { name: "Combat ended" })).toBeVisible();
    await shot(page, testInfo, "ended");
    // The rig's admin isn't a DM (the Encounters tab would send it home), so the link's target is checked.
    await expect(note.getByRole("link", { name: "Back to encounters" })).toHaveAttribute("href", /#\/encounters\/p1\/e1$/);
  });

  test("when every enemy is down it offers End combat but never ends by itself; Keep going hides the hint", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await started(page, testInfo);
    // The DM takes every creature down, typing big damage in the roster.
    for (const id of ["knight", "warhorse", "dragon", "priest", "cultist1", "cultist2", "mage", "druid"]) {
      await page.locator(`[data-combatant="${id}"] .cb-row-main`).click();
      const card = page.locator("#cb-card");
      await card.getByLabel(/Damage or healing/).fill("999");
      await card.getByRole("button", { name: "Damage", exact: true }).click();
    }
    const hint = page.getByRole("status", { name: "All enemies are down" });
    await expect(hint).toBeVisible();
    await hint.scrollIntoViewIfNeeded();
    await shot(page, testInfo, "hint");
    await page.waitForTimeout(500);
    await expect(page.getByRole("heading", { name: "Combat ended" })).toHaveCount(0);
    await hint.getByRole("button", { name: "Keep going" }).click();
    await expect(hint).toBeHidden();
  });
});
