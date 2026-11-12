import { ACCOUNTS } from "./accounts";
import { expect, keyboardHeight, open, openKeyboard, scrollIntoReach, signedInShell, test } from "./fixtures";

/**
 * The sign-in page (ConnectPage) on a phone, signed out. Plan 2.3 item 4: the card clips under the keyboard and in
 * landscape, with no way to scroll to it; M1 makes `.party-connect` a scroller padded by `--kb`.
 */
test.describe("sign-in page", () => {
  test.beforeEach(async ({ page }) => {
    await open(page, "/");
    await expect(page.getByRole("button", { name: "Sign in" })).toBeAttached();
  });

  test("Sign in can be reached without a keyboard", async ({ page }, testInfo) => {
    // M1 (0.1.5): in landscape the card is taller than the screen and the page does not scroll.
    test.fail(testInfo.project.name === "android-landscape", "M1: the sign-in card clips in landscape and cannot scroll");
    const { height } = page.viewportSize()!;
    expect(await scrollIntoReach(page, page.getByRole("button", { name: "Sign in" }), height), "Sign in is on screen or can be scrolled to").toBe(true);
  });

  test("no text field takes focus by itself (it would open the keyboard)", async ({ page }) => {
    // M1 (0.1.5): no autoFocus under html.phone (ConnectPage.tsx).
    test.fail(true, "M1: the username field has autoFocus on phones");
    const focused = await page.evaluate(() => document.activeElement?.tagName ?? "");
    expect(focused).not.toBe("INPUT");
  });

  for (const mode of ["resize", "inset"] as const) {
    const how = mode === "resize" ? "a browser shrinks the page" : "the app reports --kb";
    test(`signs in with the keyboard open (${how})`, async ({ page }, testInfo) => {
      // M1 (0.1.5): `.party-connect` neither scrolls nor pads itself by --kb, and the root does not scroll
      // (base.css). A shrunk page still fits in portrait; in landscape, or with the app's --kb, Sign in is out of reach.
      test.fail(mode === "inset" || testInfo.project.name === "android-landscape",
        "M1: the sign-in card is hidden under the keyboard and cannot be scrolled to");
      const name = page.getByLabel("Username or email");
      await name.focus();
      const { visibleBottom } = await openKeyboard(page, keyboardHeight(page), mode);
      await name.fill("player");
      await page.getByLabel("Password").fill(ACCOUNTS.player.password);
      const button = page.getByRole("button", { name: "Sign in" });
      const reach = await scrollIntoReach(page, button, visibleBottom);
      expect(reach, `Sign in is on screen above a ${keyboardHeight(page)} px keyboard, or can be scrolled there`).toBe(true);
      await button.tap();
      await signedInShell(page);
      await expect(page.locator(".party-bar")).toContainText("player");
    });
  }

  test("signs in by tapping Sign in", async ({ page }) => {
    await page.getByLabel("Username or email").fill("admin");
    await page.getByLabel("Password").fill(ACCOUNTS.admin.password);
    await page.getByRole("button", { name: "Sign in" }).tap();
    await signedInShell(page);
    await expect(page.getByRole("button", { name: "Admin" })).toBeVisible();
  });

  test("a wrong password says so and stays on the page", async ({ page }) => {
    // Not a seeded name, so the failed-sign-in limit never locks out an account the other tests use.
    await page.getByLabel("Username or email").fill("nobody");
    await page.getByLabel("Password").fill("not-the-password-1234");
    await page.getByLabel("Password").press("Enter");
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(page.locator(".party-bar")).toHaveCount(0);
  });
});
