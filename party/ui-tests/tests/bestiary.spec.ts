import { expect, measureOverflow, minTap, open, signedInShell, test } from "./fixtures";

/**
 * Additional Content on a phone: the Bestiary section (ui/party/content/BestiaryCoverage.tsx) lists the bundled SRD
 * bestiaries as complete, opens a source's counts with a tap, and never makes the page scroll sideways.
 */
test("the Bestiary section shows the SRD bestiaries as complete", async ({ page, signIn }, testInfo) => {
  await signIn("player");
  await open(page, "/content");
  await signedInShell(page);

  const list = page.getByRole("list", { name: "Bestiary by source" });
  await list.scrollIntoViewIfNeeded();
  const srd = list.getByRole("listitem", { name: "SRD 5.2.1 Monsters" });
  await expect(srd).toContainText("Complete", { timeout: 30_000 });
  await expect(srd).toContainText("330 of 330 creatures");
  await expect(list.getByRole("listitem", { name: "SRD 5.1 Monsters" })).toContainText("317 of 317 creatures");

  const toggle = srd.getByRole("button", { name: /SRD 5\.2\.1 Monsters/ });
  const box = (await toggle.boundingBox())!;
  expect(box.height, "source toggle tap target").toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
  await toggle.tap();
  await expect(srd.getByRole("group", { name: "By challenge rating" })).toBeVisible();
  await expect(srd.getByRole("group", { name: "By creature type" })).toBeVisible();

  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath("bestiary.png") });
});
