import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Browser, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, settled, smallTargets, test } from "./fixtures";

/**
 * The encounter builder (release 0.3.0) on the phones, 384x832 (Android) and 390x844 (iPhone): `admin` runs a party
 * as its DM and builds an encounter from the bundled SRD monsters, `player` plays a character in it.
 * - The DM's list and builder fit the screen, every control is a full-size tap target, and the XP bar stays at the
 *   bottom while the builder scrolls.
 * - The picker is a full-screen sheet with chips. Its search is part A's `/api/bestiary/search`; until that is on
 *   the server this spec answers it in the browser with SRD monsters, and adding goes to the real server.
 * - The player sees "Next: <name>" only once the DM shows the encounter, and only revealed creatures, never the DM's
 *   name for it or a hidden one.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
const GOBLIN = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const OWLBEAR = "ID_SRD521_MONSTER_OWLBEAR";

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

async function phone(browser: Browser, testInfo: TestInfo, baseURL: string, session: { token: string; user: unknown }): Promise<Page> {
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = testInfo.project.use;
  const context = await browser.newContext({ baseURL, viewport, userAgent, deviceScaleFactor, isMobile, hasTouch });
  await context.addInitScript((s) => { localStorage.setItem("party.session", s); }, JSON.stringify({ baseUrl: "", ...session }));
  return context.newPage();
}

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers.filter((s) => !s.includes("enc-chips")), "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-encounters-${name}.png`) });
}

/** Part A's bestiary search, answered in the browser with SRD monsters while the server has none. */
async function bestiary(page: Page) {
  const items = [
    { id: GOBLIN, name: "Goblin Warrior", cr: "1/4", xp: 50, type: "fey", size: ["small"], ac: 15, hp: 10, source: "SRD 5.2.1 Monsters", ruleset: "dnd2024", legendary: false },
    { id: OWLBEAR, name: "Owlbear", cr: "3", xp: 700, type: "monstrosity", size: ["large"], ac: 13, hp: 59, source: "SRD 5.2.1 Monsters", ruleset: "dnd2024", legendary: false },
  ];
  await page.route("**/api/bestiary/search", async (route) => {
    const q = (route.request().postDataJSON() ?? {}) as { text?: string; types?: string[] };
    const hits = items.filter((m) => (!q.text || m.name.toLowerCase().includes(q.text.toLowerCase())) && (!q.types?.length || q.types.includes(m.type)));
    await route.fulfill({ json: { total: hits.length, offset: 0, limit: 30, items: hits } });
  });
  await page.route("**/api/bestiary/facets", (route) => route.fulfill({
    json: { total: 2, types: [{ value: "fey", count: 1 }, { value: "monstrosity", count: 1 }], sizes: [], crs: [], rulesets: [], sources: [] },
  }));
}

test.describe("Encounters (0.3.0)", () => {
  test("the DM builds an encounter on the phone and the player sees only what is revealed", async ({ page, signIn, request, browser, baseURL }, testInfo) => {
    test.skip(testInfo.project.name === "android-landscape", "portrait phones: 384x832 and 390x844");
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const partyName = `Enc ${testInfo.project.name} ${Math.random().toString(36).slice(2, 7)}`;
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: partyName });
    const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
    const hero = await api<{ summary: { id: string } }>(request, player.token, "POST", "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
    await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero.summary.id });
    const enc = await api<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "The DM's secret name", publicName: "Goblin ambush" });
    await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: GOBLIN, count: 3 });

    try {
      // The DM's list.
      await signIn("admin");
      await bestiary(page);
      await open(page, `/parties/${party.id}/encounters`);
      const row = page.getByRole("link", { name: "Open The DM's secret name" });
      await expect(row).toBeVisible({ timeout: 30_000 });
      await expect(row).toContainText("3 creatures (3 hidden)");
      await fits(page, testInfo, ".enc-page");
      await shot(page, testInfo, "list");

      // The builder.
      await row.tap();
      await expect(page.getByRole("heading", { name: "The DM's secret name" })).toBeVisible();
      const bar = page.getByRole("status", { name: "Difficulty" });
      await expect(bar).toContainText("150 XP");

      // The picker: full screen, with chips; two Owlbears into the encounter.
      await page.getByRole("button", { name: /Add monsters/ }).tap();
      const picker = page.getByRole("dialog", { name: "Add monsters" });
      await settled(picker);
      const box = (await picker.boundingBox())!;
      const view = page.viewportSize()!;
      expect(box.width, "the picker is full width").toBeGreaterThanOrEqual(view.width - 1);
      expect(box.height, "the picker is full height").toBeGreaterThanOrEqual(view.height * 0.9);
      await picker.getByRole("button", { name: "Monstrosity" }).tap();
      await expect(picker.getByRole("button", { name: "Pick Goblin Warrior" })).toHaveCount(0);
      const pick = picker.getByRole("button", { name: "Pick Owlbear" });
      await pick.tap();
      await pick.tap();
      await fits(page, testInfo, ".enc-picker");
      await shot(page, testInfo, "picker");
      await picker.getByRole("button", { name: "Add 2 creatures" }).tap();
      await expect(picker).toHaveCount(0);
      // 3 x 50 + 2 x 700.
      await expect(bar).toContainText("1,550 XP");

      // Reveal one Owlbear by name.
      const owlbear = page.getByRole("listitem", { name: "Owlbear 1" });
      await owlbear.getByRole("switch", { name: "Owlbear 1: shown to players" }).tap();
      await expect(owlbear).toContainText('Players see "Monstrosity"');
      await fits(page, testInfo, ".enc-builder");

      // The bar stays at the bottom of the screen while the builder scrolls.
      await page.getByRole("button", { name: "Delete encounter" }).scrollIntoViewIfNeeded();
      // The bar's bottom is the bottom of the area that scrolls (above the phone's tab bar), and on screen.
      const placed = await bar.evaluate((el) => {
        let p = el.parentElement;
        while (p && !/(auto|scroll)/.test(getComputedStyle(p).overflowY)) p = p.parentElement;
        return { bar: el.getBoundingClientRect().bottom, scroller: (p ?? document.documentElement).getBoundingClientRect().bottom };
      });
      expect(placed.bar, "the XP bar is on screen").toBeLessThanOrEqual(view.height + 1);
      expect(Math.abs(placed.bar - placed.scroller), `the XP bar sits at the bottom of the scrolling area (${JSON.stringify(placed)})`).toBeLessThanOrEqual(2);
      await shot(page, testInfo, "builder");

      // The player: nothing yet; then "Next:" once it is shown, and only the revealed Owlbear.
      const playerPage = await phone(browser, testInfo, baseURL!, player);
      try {
        await open(playerPage, "/parties");
        const card = playerPage.getByRole("region", { name: partyName });
        await expect(card).toBeVisible();
        await expect(card.getByText("Next:")).toHaveCount(0);

        await page.getByRole("button", { name: "Delete encounter" }).scrollIntoViewIfNeeded();
        await page.getByRole("switch", { name: "Shown to players", exact: true }).first().scrollIntoViewIfNeeded();
        await page.getByRole("switch", { name: "Shown to players", exact: true }).first().tap();
        await page.getByRole("dialog").getByRole("button", { name: "Show it" }).tap();
        await expect(page.getByRole("switch", { name: "Shown to players", exact: true }).first()).toHaveAttribute("aria-checked", "true");

        const next = card.getByRole("link", { name: "Next: Goblin ambush" });
        await expect(next).toBeVisible({ timeout: 5_000 });
        await fits(playerPage, testInfo, ".party-parties");
        await next.tap();
        const seen = playerPage.getByLabel("Goblin ambush");
        await expect(seen.getByText("Monstrosity", { exact: true })).toBeVisible();
        const text = (await playerPage.locator("main").textContent()) ?? "";
        expect(text).not.toContain("secret name");
        expect(text).not.toContain("Goblin Warrior");
        expect(text).not.toContain("Owlbear");
        expect(text).not.toContain("XP");
        await fits(playerPage, testInfo, ".enc-page");
        await shot(playerPage, testInfo, "player");
      } finally {
        await playerPage.context().close();
      }
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
