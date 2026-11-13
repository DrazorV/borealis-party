import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Browser, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, smallTargets, test } from "./fixtures";

/**
 * The live party (release 0.2.1) with two accounts at once, each in its own phone: `admin` runs the party as its DM,
 * `player` plays a character in it. The DM's roster follows the player's damage within about two seconds without a
 * reload, the DM's read-only sheet follows it too, and the player sees the DM's character only as name, class, level
 * and health band (plan D6). The S26 Ultra's portrait size (384x832) only: the other phones run the 0.2.0 specs.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
/** "About two seconds", with room for a debug server the other specs keep busy. */
const LIVE_MS = 2_500;

async function login(request: APIRequestContext, who: AccountName): Promise<{ token: string; user: unknown }> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as { token: string; user: unknown };
}

async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

/** A fresh character for `auth`'s account, open as a player at the table has it. */
async function newCharacter(request: APIRequestContext, auth: string): Promise<string> {
  const report = await api<{ summary: { id: string } }>(request, auth, "POST", "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
  const id = report.summary.id;
  await api(request, auth, "POST", "/api/rpc/load_character", { characterId: id });
  return id;
}

/** A phone of the same kind as the test's, signed in as `who` (the second account's phone). */
async function phone(browser: Browser, testInfo: TestInfo, baseURL: string, session: { token: string; user: unknown }): Promise<Page> {
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = testInfo.project.use;
  const context = await browser.newContext({ baseURL, viewport, userAgent, deviceScaleFactor, isMobile, hasTouch });
  await context.addInitScript((s) => { localStorage.setItem("party.session", s); }, JSON.stringify({ baseUrl: "", ...session }));
  return context.newPage();
}

const hitPoints = /^Hit points (\d+) of (\d+)/;

/** With PARTY_SHOTS=<folder>, a screenshot for looking at by eye. */
async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-live-${name}.png`) });
}

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
}

test.describe("The live party (0.2.1)", () => {
  test("the DM sees the player's damage live, on the roster and on the read-only sheet; the player sees D6 only", async ({ page, signIn, request, browser, baseURL }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "two phones at 384x832 only");
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const name = `Live ${Math.random().toString(36).slice(2, 8)}`;
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name });
    // The DM's own character is in it too (D4), so the player has another member to look at.
    const dmPc = await newCharacter(request, dm.token);
    await api(request, dm.token, "POST", `/api/parties/${party.id}/members`, { characterId: dmPc });
    const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
    const hero = await newCharacter(request, player.token);
    await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero });

    // The DM's phone: the player's card with its numbers.
    await signIn("admin");
    await open(page, "/parties");
    const card = page.getByRole("region", { name });
    const member = card.getByRole("listitem").filter({ hasText: "player" }).first();
    const hp = member.getByRole("listitem", { name: hitPoints });
    await expect(hp).toBeVisible();
    const [, now, max] = hitPoints.exec((await hp.getAttribute("aria-label"))!)!.map(Number) as [number, number, number];
    expect(max, "the imported hero has hit points").toBeGreaterThan(1);
    await expect(member.getByRole("listitem", { name: /^Armor class \d+/ })).toBeVisible();
    await expect(member.getByRole("listitem", { name: /^Passive Perception \d+/ })).toBeVisible();
    await member.scrollIntoViewIfNeeded();
    await fits(page, testInfo, ".party-parties");
    await shot(page, testInfo, "dm-roster");

    // The player plays on their own phone: one point of damage. The DM's roster shows it without a reload.
    const playerPage = await phone(browser, testInfo, baseURL!, player);
    try {
      await open(playerPage, "/parties");
      const theirs = playerPage.getByRole("region", { name });
      await expect(theirs).toBeVisible();

      const started = Date.now();
      await api(request, player.token, "POST", "/api/rpc/take_damage", { characterId: hero, amount: 1 });
      await expect(member.getByRole("listitem", { name: new RegExp(`^Hit points ${now - 1} of ${max}`) })).toBeVisible({ timeout: LIVE_MS });
      testInfo.annotations.push({ type: "live", description: `roster updated in ${Date.now() - started} ms` });

      // D6 on the player's phone: the DM's character is a name, class, level and band; no numbers, no sheet.
      const dmCard = theirs.getByRole("listitem").filter({ hasText: "the DM's character" });
      await expect(dmCard).toBeVisible();
      await expect(dmCard.getByText(/Unhurt|Bloodied|Down/)).toBeVisible();
      await expect(dmCard.getByRole("list", { name: "Numbers" })).toHaveCount(0);
      await expect(theirs.getByRole("link", { name: /^View / })).toHaveCount(0);
      // Their own card is whole.
      await expect(theirs.getByRole("listitem").filter({ hasText: "your character" }).getByRole("listitem", { name: hitPoints })).toBeVisible();
      await fits(playerPage, testInfo, ".party-parties");
      await dmCard.scrollIntoViewIfNeeded();
      await shot(playerPage, testInfo, "player-roster");

      // The DM opens the player's read-only sheet; it follows the next damage too.
      await member.getByRole("link", { name: /^View / }).tap();
      await expect(page.getByText("Read-only")).toBeVisible();
      const sheetHp = page.getByRole("group", { name: new RegExp(`^Hit points ${now - 1} of ${max}`) });
      await expect(sheetHp).toBeVisible();
      await expect(page.getByRole("textbox")).toHaveCount(0);
      await fits(page, testInfo, ".party-sheet");
      await api(request, player.token, "POST", "/api/rpc/take_damage", { characterId: hero, amount: 1 });
      await expect(page.getByRole("group", { name: new RegExp(`^Hit points ${now - 2} of ${max}`) })).toBeVisible({ timeout: LIVE_MS + 500 });
      await shot(page, testInfo, "dm-sheet");

      // Removed: the player's phone loses the party at once, and hears nothing more of it.
      await page.getByRole("link", { name: "Back to parties" }).tap();
      await page.getByRole("region", { name }).getByRole("listitem").filter({ hasText: "player" }).first().getByRole("button", { name: "Remove" }).tap();
      await page.getByRole("dialog").getByRole("button", { name: "Remove" }).tap();
      await expect(playerPage.getByRole("region", { name })).toHaveCount(0, { timeout: LIVE_MS });
    } finally {
      await playerPage.context().close();
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
