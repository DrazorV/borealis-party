import { ACCOUNTS } from "./accounts";
import { expect, keyboardHeight, open, openKeyboard, scrollIntoReach, signedInShell, test } from "./fixtures";

/**
 * The sign-in page (ConnectPage) on a phone, signed out. Plan 2.3 M1: `.party-connect` is a scroller padded by
 * `--kb`, so the card can be reached in landscape and above the keyboard, and no field takes focus by itself.
 */
test.describe("sign-in page", () => {
  test.beforeEach(async ({ page }) => {
    await open(page, "/");
    await expect(page.getByRole("button", { name: "Sign in" })).toBeAttached();
  });

  test("Sign in can be reached without a keyboard", async ({ page }) => {
    // In landscape the card is taller than the screen: the page scrolls to it.
    const { height } = page.viewportSize()!;
    expect(await scrollIntoReach(page, page.getByRole("button", { name: "Sign in" }), height), "Sign in is on screen or can be scrolled to").toBe(true);
  });

  test("no text field takes focus by itself (it would open the keyboard)", async ({ page }) => {
    // No autoFocus under html.phone (ConnectPage.tsx).
    const focused = await page.evaluate(() => document.activeElement?.tagName ?? "");
    expect(focused).not.toBe("INPUT");
  });

  for (const mode of ["resize", "inset"] as const) {
    const how = mode === "resize" ? "a browser shrinks the page" : "the app reports --kb";
    test(`signs in with the keyboard open (${how})`, async ({ page }) => {
      // The root never scrolls (base.css): `.party-connect` scrolls itself, and pads its bottom by the app's --kb.
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
    // On a phone the way to the admin page is a More row (plan D10).
    await page.getByTestId("nav-more").tap();
    await expect(page.getByRole("link", { name: "Admin", exact: true })).toBeVisible();
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
