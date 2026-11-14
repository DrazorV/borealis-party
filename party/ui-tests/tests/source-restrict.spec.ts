import type { Page, TestInfo } from "@playwright/test";
import { expect, measureOverflow, signedInShell, test } from "./fixtures";

/**
 * Sources page "Restrict playtest content" / "Restrict third-party content" (0.3.3): two toggle buttons in the source
 * toolbar. On, they uncheck the matching books and say how many; the choice is kept in the account's settings, so it is
 * still on after a reload. The bundled books are all official, so the server's book list gets three made-up books here
 * (two playtest, one third-party); the settings are the real server's. Run at 1440x900 and on the 384x832 phone, one
 * account each, so the two runs never share settings.
 */

const MADE_UP = [
  { id: "Playtest Pages One", playtest: true, thirdParty: false },
  { id: "Playtest Pages Two", playtest: true, thirdParty: false },
  { id: "Friendly Press Guide", playtest: false, thirdParty: true },
].map((b) => ({
  ...b, name: b.id, group: "Made-up Books", packId: "made-up-books", elementCount: 3, description: "A made-up book.",
  incomplete: false, info: "", edition: null, editionSetByUser: false, editionEditable: false,
}));

const wide = (testInfo: TestInfo) => testInfo.project.name === "desktop-wide";

async function openSources(page: Page, testInfo: TestInfo) {
  await page.route(/\/api\/rpc\/list_sources$/, async (route) => {
    const real = await route.fetch();
    const list = (await real.json()) as unknown[];
    await route.fulfill({ response: real, json: [...list, ...MADE_UP] });
  });
  await page.goto(wide(testInfo) ? "/#/content/sources" : "/?mock=mobile#/content/sources");
  await signedInShell(page);
  await expect(page.getByRole("grid", { name: "Sources" })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("checkbox", { name: "Allow Friendly Press Guide" })).toBeAttached({ timeout: 30_000 });
}

async function press(page: Page, name: string, testInfo: TestInfo) {
  const b = page.getByRole("button", { name });
  if (wide(testInfo)) await b.click();
  else await b.tap();
}

test.beforeEach(async ({ signIn }, testInfo) => {
  test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "1440x900 and the 384x832 phone only");
  await signIn(wide(testInfo) ? "admin" : "player");
});

test("the restrict toggles uncheck their books, count them and stay on after a reload", async ({ page }, testInfo) => {
  await openSources(page, testInfo);
  const playtest = page.getByRole("button", { name: "Restrict playtest content" });
  const thirdParty = page.getByRole("button", { name: "Restrict third-party content" });
  const note = page.getByRole("status", { name: "Restricted sources" });
  const allow = (name: string) => page.getByRole("checkbox", { name: `Allow ${name}` });

  await expect(playtest).toHaveAttribute("aria-pressed", "false");
  await expect(thirdParty).toHaveAttribute("aria-pressed", "false");
  await expect(playtest).toHaveAttribute("title", /uncheck the 2 playtest sources/);

  await press(page, "Restrict playtest content", testInfo);
  await expect(playtest).toHaveAttribute("aria-pressed", "true");
  await expect(note).toHaveText("2 playtest sources restricted");
  await expect(allow("Playtest Pages One")).not.toBeChecked();
  await expect(allow("Friendly Press Guide")).toBeChecked();

  await press(page, "Restrict third-party content", testInfo);
  await expect(thirdParty).toHaveAttribute("aria-pressed", "true");
  await expect(note).toHaveText("2 playtest sources restricted · 1 third-party source restricted");
  await expect(allow("Friendly Press Guide")).not.toBeChecked();
  await page.screenshot({ path: testInfo.outputPath("toggles-on.png") });

  const overflow = await measureOverflow(page);
  expect(overflow.page, "no sideways scroll").toBeLessThanOrEqual(0);

  await page.reload();
  await signedInShell(page);
  await expect(page.getByRole("button", { name: "Restrict playtest content" })).toHaveAttribute("aria-pressed", "true", { timeout: 30_000 });
  await expect(page.getByRole("button", { name: "Restrict third-party content" })).toHaveAttribute("aria-pressed", "true");
  await expect(allow("Playtest Pages Two")).not.toBeChecked();

  // Off again: the books are allowed again (and the account is back to the defaults for other specs).
  await press(page, "Restrict playtest content", testInfo);
  await press(page, "Restrict third-party content", testInfo);
  await expect(playtest).toHaveAttribute("aria-pressed", "false");
  await expect(thirdParty).toHaveAttribute("aria-pressed", "false");
  await expect(allow("Playtest Pages One")).toBeChecked();
  await expect(allow("Friendly Press Guide")).toBeChecked();
  await expect(note).toHaveText("");
});
