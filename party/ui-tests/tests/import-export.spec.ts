import { expect, open, scrollIntoReach, signedInShell, test } from "./fixtures";

/**
 * The ways out of an account on a phone (plan 2.2) can be found and tapped. Nothing is exported here: the entry point
 * only has to open its dialog. The ways in (Import characters, Import files, Open bundle... and the rest) all lead to
 * the Import screen since 0.1.5: unify-imports.spec.ts.
 */
test.describe("export entry points", () => {
  test.beforeEach(async ({ page, signIn }) => {
    await signIn("player");
  });

  test("Menu: Export to bundle... opens the export dialog, which closes again", async ({ page }) => {
    // Chrome on Android (these projects: Android emulation, no Tauri) shows the row like every browser. Only the
    // Android app hides it (phoneOs() === "android" && isTauri()); that case is covered by vitest:
    // api/overrides/android.test.ts and entry/hideTransfer.test.tsx.
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
