import { expect, finding, measureOverflow, minTap, open, reportSmallTargets, signedInShell, smallTargets, test } from "./fixtures";
import { MAIN_PAGES } from "./pages";

/**
 * Every main screen fits the phone's width: the page never scrolls sideways (a hard check). Party controls (the
 * account bar, the More rows, the sign-in card, the account and admin pages) are at least a comfortable tap target
 * (44 px iOS, 48 px Android; plan M3, D9), also a hard check. Content cut off at the right edge and upstream's
 * small controls are reported in findings.md without failing.
 */
test.describe("fits the screen", () => {
  test("sign-in page", async ({ page }, testInfo) => {
    await open(page, "/");
    await expect(page.getByRole("button", { name: "Sign in" })).toBeAttached();
    await check(page, testInfo);
  });

  for (const p of MAIN_PAGES) {
    test(p.name, async ({ page, signIn }, testInfo) => {
      await signIn("admin");
      await open(page, p.route);
      await signedInShell(page);
      await page.waitForTimeout(500); // late layout: the app note, lists filling in
      await check(page, testInfo);
    });
  }
});

test.describe("phone top bar and More (plan D10)", () => {
  test("the top bar has no Account, Admin or Sign out; More has them as rows", async ({ page, signIn }) => {
    await signIn("admin");
    await open(page, "/characters");
    await signedInShell(page);
    // A browser keeps no copy, so there is no Sync chip, and so no bar on screen.
    await expect(page.locator(".party-bar")).toBeHidden();
    for (const name of ["Account", "Admin", "Sign out"]) await expect(page.locator(".party-bar").getByRole("button", { name })).toHaveCount(0);
    await page.getByTestId("nav-more").tap();
    const rows = page.getByRole("list", { name: "Account" });
    await expect(rows.getByRole("link", { name: /^Account/ })).toBeVisible();
    await expect(rows.getByRole("link", { name: "Admin", exact: true })).toBeVisible();
    await expect(rows.getByRole("button", { name: "Sign out" })).toBeVisible();
    // The account's rows come first, above upstream's.
    const first = await page.locator(".v5-list-pane .v5-list").first().getAttribute("aria-label");
    expect(first).toBe("Account");
    await rows.getByRole("link", { name: /^Account/ }).tap();
    await expect(page.getByText("My account")).toBeVisible();
  });

  test("a player's More has no Admin row, and Sign out signs out", async ({ page, signIn }) => {
    await signIn("player");
    await open(page, "/more");
    await signedInShell(page);
    const rows = page.getByRole("list", { name: "Account" });
    await expect(rows.getByRole("button", { name: "Sign out" })).toBeVisible();
    await expect(rows.getByRole("link", { name: "Admin", exact: true })).toHaveCount(0);
    // A browser keeps no copy on the device, so there is nothing to ask about.
    await rows.getByRole("button", { name: "Sign out" }).tap();
    await expect(page.getByRole("button", { name: "Sign in" })).toBeVisible({ timeout: 30_000 });
  });
});

/** Party-owned parts of a page: their controls must meet the tap size (D9). */
const PARTY_PARTS = [".party-bar-host", ".party-more", ".party-connect", ".party"];

async function check(page: import("@playwright/test").Page, testInfo: import("@playwright/test").TestInfo): Promise<void> {
  const o = await measureOverflow(page);
  for (const c of o.clipped) finding(testInfo, `cut off: ${c}`);
  const small = await reportSmallTargets(page, testInfo);
  await testInfo.attach("layout.json", { body: JSON.stringify({ overflow: o, smallTargets: small }, null, 2), contentType: "application/json" });
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways only because their content is too wide").toEqual([]);
  const party: string[] = [];
  for (const part of PARTY_PARTS) {
    // smallTargets looks at the whole page when the part is missing.
    if ((await page.locator(part).count()) === 0) continue;
    for (const t of await smallTargets(page, minTap(testInfo), part)) party.push(`${part}: ${t.label} ${t.width}x${t.height}`);
  }
  expect([...new Set(party)], `party controls under ${minTap(testInfo)} px`).toEqual([]);
}
