import { expect, finding, measureOverflow, open, reportSmallTargets, signedInShell, test } from "./fixtures";
import { MAIN_PAGES } from "./pages";

/**
 * Every main screen fits the phone's width: the page never scrolls sideways (a hard check). Content cut off at the
 * right edge and controls smaller than a comfortable tap target (44 px iOS, 48 px Android; plan M3) are reported in
 * findings.md without failing, since wave 3 fixes them.
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

async function check(page: import("@playwright/test").Page, testInfo: import("@playwright/test").TestInfo): Promise<void> {
  const o = await measureOverflow(page);
  for (const c of o.clipped) finding(testInfo, `cut off: ${c}`);
  const small = await reportSmallTargets(page, testInfo);
  await testInfo.attach("layout.json", { body: JSON.stringify({ overflow: o, smallTargets: small }, null, 2), contentType: "application/json" });
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways only because their content is too wide").toEqual([]);
}
