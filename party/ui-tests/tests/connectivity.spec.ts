import { expect, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * Losing and regaining the signal on a phone (plan M6): the banner says "Reconnecting…" and then "Back online", the
 * page never reloads under the user, and nothing typed is lost. A browser here; the apps' offline switch is covered by
 * the unit tests (ui/party/api/connectivity.test.ts).
 */
test.describe("connectivity", () => {
  test.beforeEach(async ({ signIn }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "one phone (384x832) is enough for the network states");
    await signIn("player");
  });

  test("losing the signal shows Reconnecting…, getting it back shows Back online, and nothing reloads", async ({ page, context }) => {
    await open(page, "/characters");
    await signedInShell(page);
    // A mark on this page: a reload would wipe it.
    await page.evaluate(() => { (window as unknown as { phoneRigMark: number }).phoneRigMark = 1; });
    const banner = page.locator(".party-offline");
    await expect(banner).toHaveCount(0);

    await context.setOffline(true);
    await expect(banner).toContainText("Reconnecting…", { timeout: 15_000 });
    await expect(banner.getByRole("button", { name: "Try now" })).toBeVisible();
    const overflow = await measureOverflow(page);
    expect(overflow.page, "the banner doesn't make the page scroll sideways").toBeLessThanOrEqual(1);

    // Something typed while the signal is gone stays where it is.
    const field = await page.evaluate(() => {
      const input = document.createElement("input");
      input.id = "phone-rig-field";
      // Not in .party-bar: on phones that row is hidden when it has no Sync chip (every browser).
      input.style.cssText = "position:fixed;left:16px;bottom:16px;z-index:9999";
      document.body.append(input);
      return input.id;
    });
    await page.locator(`#${field}`).fill("half a note");

    await context.setOffline(false);
    await expect(banner).toContainText("Back online", { timeout: 15_000 });
    await expect(page.locator(`#${field}`)).toHaveValue("half a note");
    // "Back online" goes away by itself.
    await expect(banner).toHaveCount(0, { timeout: 10_000 });
    expect(await page.evaluate(() => (window as unknown as { phoneRigMark?: number }).phoneRigMark), "the page did not reload").toBe(1);
  });
});
