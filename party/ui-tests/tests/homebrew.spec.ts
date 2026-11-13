import type { Page, TestInfo } from "@playwright/test";
import { join } from "node:path";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, minTap, open, smallTargets, test } from "./fixtures";

/**
 * Homebrew (release 0.3.1) on the phones, 384x832 (Android) and 390x844 (iPhone): `player` makes a pack, adds a
 * made-up magic item from the form, sees the engine refuse a broken one in the XML tab with its line, and builds the
 * pack's index for its own repository (the server never hosts one, D47). The pages fit the screen and every control is
 * a full-size tap target. Each project uses its own pack.
 */

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers.filter((s) => !s.includes("hb-xml")), "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-homebrew-${name}.png`) });
}

test("a pack, an element from the form, a refused one in XML, and its index", async ({ page, signIn, request, baseURL }, testInfo) => {
  test.skip(testInfo.project.name === "desktop");
  await signIn("player");
  // The pack through the API (one per project), the element through the pages.
  const login = await request.post(`${baseURL}/api/auth/login`, { data: { username: "player", password: ACCOUNTS.player.password } });
  const { token } = (await login.json()) as { token: string };
  const name = `Rig Brew ${testInfo.project.name}`;
  const made = await request.post(`${baseURL}/api/homebrew/packs`, { headers: { authorization: `Bearer ${token}` }, data: { name } });
  expect(made.ok(), await made.text()).toBeTruthy();
  const pack = ((await made.json()) as { id: string }).id;

  await open(page, `/homebrew/${pack}`);
  await expect(page.getByRole("heading", { name })).toBeVisible();
  await fits(page, testInfo, ".hb-page");
  await page.getByLabel("Element type to add").selectOption("Magic Item");
  await page.getByRole("button", { name: "Add", exact: true }).click();

  await page.getByLabel("Name", { exact: true }).fill("Lantern of Testing");
  await page.getByLabel("Description").fill("This lantern sheds bright light on hidden bugs.");
  await page.getByLabel("Rarity").selectOption("Uncommon");
  await fits(page, testInfo, ".hb-page");
  await shot(page, testInfo, "form");
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("The engine accepts it.")).toBeVisible({ timeout: 30_000 });

  // The XML tab: a broken edit is refused with the line, and nothing is saved.
  await page.getByRole("tab", { name: "XML" }).click();
  const xml = page.getByLabel("Element XML");
  await expect(xml).toHaveValue(/Lantern of Testing/);
  const broken = (await xml.inputValue()).replace("</description>", "");
  // WebKit can drop a fill that lands while React is still rendering the tab: fill until the edit is there.
  await expect(async () => {
    await xml.fill(broken);
    await expect(xml).not.toHaveValue(/<\/description>/, { timeout: 1_000 });
  }).toPass({ timeout: 15_000 });
  await page.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText(/of the element/).first()).toBeVisible({ timeout: 30_000 });
  await shot(page, testInfo, "refused");

  // The index for the author's own repository: no address, no index; with one, it shows and names that address.
  await page.getByRole("link", { name: "Back to the pack" }).click();
  await expect(page.getByRole("link", { name: "Edit Lantern of Testing" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Publish/ })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Show the index" })).toBeDisabled();
  await page.getByLabel("Hosting address").fill("https://raw.githubusercontent.com/someone/test-homebrew/main/");
  await page.getByRole("button", { name: "Show the index" }).click();
  await expect(page.getByLabel("Index preview")).toContainText("https://raw.githubusercontent.com/someone/test-homebrew/main/");
  await fits(page, testInfo, ".hb-page");
  await shot(page, testInfo, "index");
});
