import { join } from "node:path";
import type { Page, TestInfo } from "@playwright/test";
import { expect, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * 0.3.5 combat quick wins on the DM's 1440x900 screen, against the tracker's mock server (`party.combatMock`):
 * multi-target damage from the plain Damage keypad (one roll, each target's save, half on a success; a PC is asked),
 * and a late-joining party member added with their initiative. The real server's side is in
 * borealis-server/src/combat_tests.rs (quick_wins).
 */

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (!dir) return;
  // Dialogs rise in (180 ms): the shot waits for every finite animation.
  await page.evaluate(() => Promise.all(document.getAnimations().filter((a) => a.effect?.getTiming().iterations !== Infinity).map((a) => a.finished.catch(() => undefined))));
  await page.screenshot({ path: join(dir, `${testInfo.project.name}-combat-quick-wins-${name}.png`), fullPage: false });
}

async function started(page: Page, phone = false) {
  await page.addInitScript(() => { localStorage.setItem("party.combatMock", "1"); localStorage.removeItem("party.combatMock.state"); });
  if (phone) await open(page, "/combat/c1");
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
  await page.locator(".cb-thumb:visible .btn, .cb-top-actions .cb-next:visible").first().click();
  await expect(page.getByText("ROUND 1").first()).toBeVisible();
}

test.describe("Combat quick wins (0.3.5): the DM", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(testInfo.project.name !== "desktop-wide", "a DM's 1440x900 screen");
  });

  test("one damage roll on several targets, each with its own save, half on a success; the PC gets a prompt", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await started(page);
    await page.locator('[data-combatant="cultist1"] .cb-row-main').click();
    const card = page.locator("#cb-card");
    await card.getByRole("button", { name: "Several targets…" }).click();
    const sheet = page.getByRole("dialog", { name: "Damage several targets" });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByLabel("Target Cultist 1")).toBeChecked();
    await sheet.getByLabel("Damage: a total, or dice to roll once").fill("12");
    await sheet.getByLabel("Damage type").selectOption("fire");
    await sheet.getByLabel("Save DC").fill("13");
    await sheet.getByLabel("Target Cultist 2").check();
    await sheet.getByLabel("Target Priest").check();
    await sheet.getByLabel("Target Aria").check();
    // Saves are outcomes (0.3.5): the PC's asked at the table and pressed; one creature's pressed; the rest rolled.
    await sheet.locator('[data-target="pc-aria"]').getByRole("button", { name: "Saved" }).click();
    await sheet.locator('[data-target="cultist1"]').getByRole("button", { name: "Failed" }).click();
    await sheet.locator('[data-target="priest"]').getByRole("button", { name: "Saved" }).click();
    await sheet.getByRole("button", { name: "Roll creature saves (1)" }).click();
    await expect(sheet.getByRole("button", { name: "Apply to 4 targets" })).toBeEnabled();
    const o = await measureOverflow(page);
    expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
    await shot(page, testInfo, "multi-damage");
    await sheet.getByRole("button", { name: "Apply to 4 targets" }).click();
    await expect(sheet).toHaveCount(0);
    // Cultist 1 failed: 12 of its 9 HP; the priest saved: 6.
    await page.locator('[data-combatant="priest"] .cb-row-main').click();
    await expect(page.locator("#cb-card").getByRole("meter", { name: "Priest hit points" })).toHaveAttribute("aria-valuenow", "21");
    // The PC saved: half, as a prompt on the player's side; the DM sees it was sent, never Aria's HP.
    await page.locator('[data-combatant="pc-aria"] .cb-row-main').click();
    await expect(page.locator("#cb-card").getByRole("list", { name: /^Sent to/ })).toContainText("6");
    await expect(page.locator("#cb-card").getByRole("meter", { name: /Aria hit points/ })).toHaveCount(0);
    await shot(page, testInfo, "multi-damage-done");
  });

  test("a party member who joined late is added with their initiative and takes their place in the order", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await started(page);
    const roster = page.getByRole("region", { name: "Combatants" });
    await roster.getByRole("button", { name: "Party member" }).click();
    const dialog = page.getByRole("dialog", { name: "Add a party member" });
    await expect(dialog.getByRole("radio", { name: "Ilse" })).toHaveAttribute("aria-checked", "true");
    await dialog.getByLabel("Initiative: typed result").fill("11");
    await shot(page, testInfo, "add-member");
    await dialog.getByRole("button", { name: "Add to the fight" }).click();
    await expect(dialog).toHaveCount(0);
    const row = page.locator('[data-combatant="pc-c_late"]');
    await expect(row).toBeVisible();
    // Between the dragon (12) and the priest (10).
    const ids = await page.locator("[data-combatant]").evaluateAll((els) => els.map((e) => e.getAttribute("data-combatant")));
    expect(ids.indexOf("pc-c_late")).toBeGreaterThan(ids.indexOf("dragon"));
    expect(ids.indexOf("pc-c_late")).toBeLessThan(ids.indexOf("priest"));
    await expect(roster.getByRole("button", { name: "Party member" })).toHaveCount(0);
    await shot(page, testInfo, "member-added");
  });
});

test.describe("Combat quick wins (0.3.5): stat block features", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(!["android-portrait", "desktop-wide"].includes(testInfo.project.name), "the S26 Ultra and a DM's 1440x900 screen");
  });

  test("Multiattack is a feature line, not a button; the attacks it names are buttons", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await started(page, testInfo.project.name === "android-portrait");
    // The knight acts first; its card shows its turn panel.
    const card = page.locator("#cb-card");
    const line = card.locator('[data-feature="multiattack"]');
    await expect(line).toContainText("Multiattack: The knight makes two melee attacks.");
    await expect(card.getByRole("button", { name: /^Multiattack/ })).toHaveCount(0);
    await expect(card.getByRole("button", { name: /^Greatsword/ })).toBeVisible();
    await line.scrollIntoViewIfNeeded();
    expect((await measureOverflow(page)).page, "the page scrolls sideways").toBeLessThanOrEqual(0);
    await shot(page, testInfo, "multiattack");
  });
});
