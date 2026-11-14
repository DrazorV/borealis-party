import { join } from "node:path";
import type { Page, TestInfo } from "@playwright/test";
import { expect, finding, measureOverflow, minTap, open, signedInShell, smallTargets, test } from "./fixtures";

/**
 * Spells and mechanics in combat (0.3.2), the DM side: the turn panel (a button for every action), the cast sheet,
 * the resolution sheet with typed table rolls first, and a summon spell's options opening Add creature. On the S26
 * Ultra's 384x832 and on a DM's 1440x900 screen. Against the tracker's own mock server (`party.combatMock`, SRD-only
 * sample fight), so it needs no combat routes on the test server.
 */

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-spells-${name}.png`), fullPage: false });
}

const phone = (testInfo: TestInfo) => testInfo.project.name === "android-portrait" || testInfo.project.name === "iphone";

/** Opens the mock fight, types the table initiatives, starts it and steps to `who`'s turn. */
async function turnOf(page: Page, testInfo: TestInfo, who: string) {
  await page.addInitScript(() => { localStorage.setItem("party.combatMock", "1"); localStorage.removeItem("party.combatMock.state"); });
  if (phone(testInfo)) await open(page, "/combat/c1");
  else {
    await page.goto("/#/combat/c1");
    await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
  }
  await signedInShell(page);
  await expect(page.locator("[data-combatant]").first()).toBeVisible({ timeout: 30_000 });
  for (const [label, n] of [["Knight", 18], ["Adult Red Dragon", 12], ["Priest", 10], ["Cultist 1", 9], ["Cultist 2", 8], ["Mage", 5], ["Druid", 4]] as const) {
    const box = page.getByLabel(`${label} initiative`, { exact: true });
    await box.fill(String(n));
    await box.press("Enter");
  }
  const next = page.locator(".cb-thumb:visible .btn, .cb-top-actions .cb-next:visible").first();
  await next.click();
  await expect(page.getByText("ROUND 1").first()).toBeVisible();
  for (let i = 0; i < 12; i++) {
    if (await page.getByRole("heading", { name: `${who}'s turn` }).isVisible()) break;
    await next.click();
  }
  await expect(page.getByRole("heading", { name: `${who}'s turn` })).toBeVisible();
  return page.getByRole("region", { name: `${who} details` });
}

async function noSideways(page: Page) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
}

test.describe("Spells and mechanics in combat (0.3.2)", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(!["android-portrait", "desktop-wide"].includes(testInfo.project.name), "the S26 Ultra and a DM's 1440x900 screen");
  });

  test("the turn panel: a button per action, grouped, with its numbers; spending one says why the rest can't", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    const card = await turnOf(page, testInfo, "Mage");
    const panel = card.locator("[data-turn-panel]");
    await expect(panel.getByRole("group", { name: "Action economy" })).toBeVisible();
    await expect(panel.getByRole("region", { name: "Action", exact: true })).toBeVisible();
    await expect(panel.getByRole("region", { name: "Bonus action" })).toBeVisible();
    await expect(panel.getByRole("region", { name: "Reaction", exact: true })).toBeVisible();
    await expect(panel.locator('[data-turn-button="fireball"]')).toContainText("Fireball L3+");
    await expect(panel.locator('[data-turn-button="dagger"]')).toContainText("+5 · 1d4+2");
    for (const s of ["Dodge", "Dash", "Disengage", "Help"]) await expect(panel.locator(`[data-turn-button="std-${s.toLowerCase()}"]`)).toBeVisible();
    // The sticky reactions strip while the reaction is unspent.
    await expect(panel.getByRole("toolbar", { name: "Reactions ready" })).toContainText("Shield");
    await panel.scrollIntoViewIfNeeded();
    await shot(page, testInfo, "turn-panel");
    await panel.locator('[data-turn-button="std-dodge"]').click();
    await expect(panel.locator('[data-turn-button="fireball"]')).toBeDisabled();
    await expect(panel.locator(".cb-tb-item", { has: page.locator('[data-turn-button="fireball"]') })).toContainText("Action spent");
    await noSideways(page);
    if (phone(testInfo)) {
      const min = minTap(testInfo);
      const small = (await smallTargets(page, min, "[data-turn-panel]")).map((t) => `${t.label} ${t.width}x${t.height}`);
      if (small.length) finding(testInfo, `turn panel: ${small.length} controls under ${min} px: ${small.slice(0, 6).join(", ")}`);
    }
    await shot(page, testInfo, "turn-panel-spent");
  });

  test("cast and resolve: slot chips with a live preview, then typed table rolls first, Enter to the next row", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    const card = await turnOf(page, testInfo, "Mage");
    await card.locator('[data-turn-button="fireball"]').click();
    const sheet = page.getByRole("dialog", { name: "Cast Fireball" });
    await expect(sheet).toBeVisible();
    await sheet.getByRole("radio", { name: /L4/ }).click();
    await expect(sheet.locator("[data-preview]")).toHaveText("9d6 fire at L4 · DEX save, half on a success · 20-ft sphere");
    for (const t of ["Knight", "Cultist 1", "Cultist 2", "Aria"]) await sheet.getByRole("button", { name: t, exact: true }).click();
    await noSideways(page);
    await shot(page, testInfo, "cast-sheet");
    await sheet.getByRole("button", { name: "Cast", exact: true }).click();

    const res = page.getByRole("dialog", { name: "Fireball · Mage" });
    await expect(res).toBeVisible();
    // The number field is the row's main control: a number pad on phones.
    const knight = res.getByLabel("Knight: DEX save: typed result");
    await expect(knight).toHaveAttribute("inputmode", "numeric");
    await knight.fill("16");
    await knight.press("Enter");
    await expect(res.getByLabel("Cultist 1: DEX save: typed result")).toBeFocused();
    await page.keyboard.type("7");
    await page.keyboard.press("Enter");
    await expect(res.getByLabel("Cultist 2: DEX save: typed result")).toBeFocused();
    await expect(res.locator('[data-target="pc-aria"]')).toContainText("Waiting for Aria's player");
    await shot(page, testInfo, "resolution-typed");
    await page.keyboard.type("15");
    await page.keyboard.press("Enter");
    await res.locator('[data-target="pc-aria"]').getByRole("button", { name: "Type it" }).click();
    await res.getByLabel("Aria: DEX save: typed result").fill("9");
    await res.getByLabel("Aria: DEX save: typed result").press("Enter");
    const damage = res.getByLabel("Fireball damage: typed total");
    await expect(damage).toBeEnabled();
    await damage.fill("31");
    await noSideways(page);
    await shot(page, testInfo, "resolution-damage");
    await res.getByRole("button", { name: "Apply", exact: true }).click();
    await expect(res).toBeHidden();
    await expect(page.getByRole("button", { name: /pending/ })).toHaveCount(0);
    await expect(page.locator('[data-combatant="knight"]')).toContainText("37");
  });

  test("a summon spell shows its options; one opens Add creature pre-filtered, bound to the caster's concentration", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    const card = await turnOf(page, testInfo, "Druid");
    await card.locator('[data-turn-button="conjure_animals"]').click();
    await page.getByRole("dialog", { name: "Cast Conjure Animals" }).getByRole("button", { name: "Cast", exact: true }).click();
    const panel = page.getByRole("status", { name: "Conjure Animals options" });
    await expect(panel).toBeVisible();
    await expect(panel).toContainText("8 × CR 1/4 beasts…");
    await panel.scrollIntoViewIfNeeded();
    await shot(page, testInfo, "summon-options");
    await panel.getByRole("button", { name: "2 × CR 1 beasts…" }).click();
    const add = page.getByRole("dialog", { name: "Add creatures: Conjure Animals" });
    await expect(add.getByLabel("CR up to")).toHaveValue("1");
    await expect(add.getByLabel("Controlled by")).toHaveValue("druid");
    await expect(add.getByRole("checkbox", { name: /Remove when Druid's concentration on Conjure Animals ends/ })).toBeChecked();
    await add.getByRole("button", { name: /Brown Bear/ }).click();
    // The count stepper is whole on a phone too (the select beside it used to cover its +).
    const more = await add.getByRole("button", { name: "One more" }).boundingBox();
    const owner = await add.getByLabel("Controlled by").boundingBox();
    expect(more && owner && (more.x + more.width <= owner.x + 1 || more.y + more.height <= owner.y + 1), "One more is not under Controlled by").toBe(true);
    await noSideways(page);
    await shot(page, testInfo, "add-creature");
    await add.getByRole("button", { name: "Add 2 Brown Bear" }).click();
    await expect(page.locator("[data-combatant]", { hasText: "Brown Bear" })).toHaveCount(2);
    await card.getByRole("button", { name: "End concentration on Conjure Animals" }).click();
    await expect(page.locator("[data-combatant]", { hasText: "Brown Bear" })).toHaveCount(0);
  });
});
