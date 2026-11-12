import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, open, reportSmallTargets, settled, signedInShell, test } from "./fixtures";

/**
 * Dialogs and sheets open, fit the screen, and close again: with their own close button and with Escape (a hardware
 * keyboard; Android Back in the apps goes through the same overlay stack). Nothing here confirms an action: the
 * accounts are shared by the whole run.
 */
async function expectFits(page: Page, dialog: Locator, testInfo: TestInfo): Promise<void> {
  await expect(dialog).toBeVisible();
  await settled(dialog);
  const box = (await dialog.boundingBox())!;
  const { width } = page.viewportSize()!;
  expect(box.x, "dialog starts inside the screen").toBeGreaterThanOrEqual(-1);
  expect(box.x + box.width, "dialog ends inside the screen").toBeLessThanOrEqual(width + 1);
  await reportSmallTargets(page, testInfo, "[role=dialog]");
}

test.describe("dialogs", () => {
  test.beforeEach(async ({ page, signIn }) => {
    await signIn("admin");
  });

  test("Menu sheet opens and closes", async ({ page }, testInfo) => {
    await open(page, "/characters");
    await signedInShell(page);
    await page.getByRole("button", { name: "Menu" }).tap();
    const menu = page.getByRole("dialog", { name: "Menu" });
    await expectFits(page, menu, testInfo);
    // Escape closes it too (the visible Close is checked in phone-browser.spec.ts).
    await page.keyboard.press("Escape");
    await expect(menu).toBeHidden();
    await page.getByRole("button", { name: "Menu" }).tap();
    await expect(menu).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(menu).toBeHidden();
  });

  test("Menu sheet: Help has no Check for updates (desktop app only)", async ({ page }) => {
    await open(page, "/characters");
    await signedInShell(page);
    await page.getByRole("button", { name: "Menu" }).tap();
    const menu = page.getByRole("dialog", { name: "Menu" });
    await expect(menu.getByRole("button", { name: "Privacy Policy" })).toHaveCount(1);
    await expect(menu.getByRole("button", { name: /Check for updates/ })).toHaveCount(0);
  });

  test("Application Settings opens and closes", async ({ page }, testInfo) => {
    await open(page, "/characters");
    await signedInShell(page);
    await page.getByRole("button", { name: "Settings" }).first().tap();
    const settings = page.getByRole("dialog", { name: /Application Settings/ });
    await expectFits(page, settings, testInfo);
    await settings.getByRole("button", { name: "Cancel" }).tap();
    await expect(settings).toBeHidden();
    await page.getByRole("button", { name: "Settings" }).first().tap();
    await expect(settings).toBeVisible();
    await settings.getByRole("button", { name: "Close" }).tap();
    await expect(settings).toBeHidden();
  });

  test("Admin: Reset password dialog opens and closes", async ({ page }, testInfo) => {
    await open(page, "/admin");
    await signedInShell(page);
    const player = page.locator(".party-member").filter({ hasText: "player@phone-rig.test" });
    await player.getByRole("button", { name: "Reset password" }).tap();
    const dialog = page.getByRole("dialog", { name: /Reset player's password/ });
    await expectFits(page, dialog, testInfo);
    await dialog.getByRole("button", { name: "Cancel" }).tap();
    await expect(dialog).toBeHidden();
    await player.getByRole("button", { name: "Reset password" }).tap();
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
  });

  test("Admin: Delete asks first, and Cancel keeps the account", async ({ page }, testInfo) => {
    await open(page, "/admin");
    await signedInShell(page);
    const player = page.locator(".party-member").filter({ hasText: "player@phone-rig.test" });
    await player.getByRole("button", { name: "Delete" }).tap();
    const dialog = page.getByRole("dialog");
    await expectFits(page, dialog, testInfo);
    await dialog.getByRole("button", { name: "Cancel" }).tap();
    await expect(dialog).toBeHidden();
    await expect(player).toBeVisible();
  });
});
