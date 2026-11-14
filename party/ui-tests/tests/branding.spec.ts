import { expect, open, signedInShell, test } from "./fixtures";

/**
 * The product name the user reads is "Borealis Connect" (user, 2026-11-14), set at build time over upstream's text
 * (ui/party/branding.ts): the desktop title bar above the main tabs, a phone's title bar with no character open, the
 * Start page's heading and the browser tab's title.
 */
test.describe("Branding", () => {
  test("the title bar and the tab title say Borealis Connect", async ({ page, signIn }, testInfo) => {
    await signIn("player");
    if (testInfo.project.name === "desktop") await page.goto("/#/characters");
    else await open(page, "/characters");
    await signedInShell(page);
    await expect(page).toHaveTitle("Borealis Connect");
    const title = page.locator(".aur-title-text");
    await expect(title).toHaveText(/^Borealis Connect$/i);
    // The desktop bar shows it upper case, as upstream's CSS does for "BOREALIS".
    if (testInfo.project.name === "desktop") expect(await title.evaluate((e) => (e as HTMLElement).innerText)).toBe("BOREALIS CONNECT");
    await expect(page.getByRole("heading", { name: /^Welcome to Borealis Connect$/i })).toBeVisible();
    const shots = process.env.PARTY_SHOTS;
    if (shots) await page.screenshot({ path: `${shots}/${testInfo.project.name}-branding.png` });
  });
});
