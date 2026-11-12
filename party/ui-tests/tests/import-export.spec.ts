import { expect, open, scrollIntoReach, signedInShell, test } from "./fixtures";

/**
 * The ways in and out of an account on a phone (plan 2.1 and 2.2) can be found and tapped. Nothing is imported or
 * exported here: each entry point only has to open its file picker or its dialog.
 */
test.describe("import and export entry points", () => {
  test.beforeEach(async ({ page, signIn }) => {
    await signIn("player");
  });

  test("Start: Import characters opens a file picker", async ({ page }) => {
    await open(page, "/characters");
    await signedInShell(page);
    const button = page.getByRole("button", { name: /Import characters/ });
    expect(await scrollIntoReach(page, button, page.viewportSize()!.height)).toBe(true);
    const chooser = page.waitForEvent("filechooser");
    await button.tap();
    await chooser;
  });

  test("Additional Content: Import files opens a file picker", async ({ page }) => {
    await open(page, "/content");
    await signedInShell(page);
    const button = page.getByRole("button", { name: "Import files" });
    expect(await scrollIntoReach(page, button, page.viewportSize()!.height)).toBe(true);
    // Upstream's content notice may come first; it is not shown again once the account has accepted it.
    const notice = page.getByRole("dialog", { name: "Import only content you own" });
    const chooser = page.waitForEvent("filechooser");
    await button.tap();
    const first = await Promise.race([
      chooser.then(() => "picker" as const),
      notice.waitFor({ state: "visible" }).then(() => "notice" as const),
    ]);
    if (first === "notice") await notice.getByRole("button", { name: "I understand, continue" }).tap();
    await chooser;
  });

  test("Menu: Open bundle... opens a file picker", async ({ page }) => {
    await open(page, "/characters");
    await signedInShell(page);
    await page.getByRole("button", { name: "Menu" }).tap();
    const menu = page.getByRole("dialog", { name: "Menu" });
    const row = menu.getByRole("button", { name: "Open bundle..." });
    expect(await scrollIntoReach(page, row, page.viewportSize()!.height)).toBe(true);
    const chooser = page.waitForEvent("filechooser");
    await row.tap();
    await chooser;
  });

  test("Menu: Export to bundle... opens the export dialog, which closes again", async ({ page }) => {
    await open(page, "/characters");
    await signedInShell(page);
    await page.getByRole("button", { name: "Menu" }).tap();
    const menu = page.getByRole("dialog", { name: "Menu" });
    const row = menu.getByRole("button", { name: "Export to bundle..." });
    expect(await scrollIntoReach(page, row, page.viewportSize()!.height)).toBe(true);
    await row.tap();
    // On a phone upstream calls the export "Share as a file".
    const dialog = page.getByRole("dialog", { name: /Share as a file|Export/ });
    await expect(dialog).toBeVisible();
    const box = (await dialog.boundingBox())!;
    expect(box.x + box.width).toBeLessThanOrEqual(page.viewportSize()!.width + 1);
    const cancel = dialog.getByRole("button", { name: "Cancel" });
    expect(await scrollIntoReach(page, cancel, page.viewportSize()!.height), "the export dialog's Cancel can be reached").toBe(true);
    await cancel.tap();
    await expect(dialog).toBeHidden();
  });
});
