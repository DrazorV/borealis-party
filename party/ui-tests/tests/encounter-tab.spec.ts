import { join } from "node:path";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, SEEDED } from "./accounts";
import { expect, measureOverflow, minTap, open, settled, smallTargets, test } from "./fixtures";

/**
 * The DM's Encounters and Combat main tabs (0.3.1, plans 4.3 and 4.5b), on a big screen (1440x900, the `desktop-wide`
 * project) and on the phone (384x832):
 * - only a DM has the tabs, and the routes send anyone else to the start page;
 * - on a big screen the Encounters tab uses the whole width: the party's encounters on the left, the creatures in the
 *   middle, the picker full width under them (0.3.5; keyboard: / opens it, Enter adds), players and setup on the right, and no
 *   bottom bar; the Combat tab lists ready encounters to start and opens the tracker full width;
 * - on the phone both tabs are rows on More, the builder keeps its stacked layout and its summary bar, and nothing
 *   scrolls sideways. No XP or difficulty anywhere (D16).
 * Screenshots go to $PARTY_SHOTS when it is set.
 */

const GOBLIN = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const OWLBEAR = "ID_SRD521_MONSTER_OWLBEAR";

async function login(request: APIRequestContext, username: string, password: string): Promise<{ token: string; user: unknown }> {
  const r = await request.post("/api/auth/login", { data: { username, password } });
  expect(r.ok(), `sign in as ${username}`).toBeTruthy();
  return (await r.json()) as { token: string; user: unknown };
}

async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

async function seed(page: Page, session: { token: string; user: unknown }) {
  await page.addInitScript((s) => {
    if (!sessionStorage.getItem("enc-tab.seeded")) {
      localStorage.setItem("party.session", s);
      sessionStorage.setItem("enc-tab.seeded", "1");
    }
  }, JSON.stringify({ baseUrl: "", ...session }));
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-encounter-tab-${name}.png`) });
}

/** The bestiary search answered in the browser with SRD monsters, as encounters.spec.ts does. */
async function bestiary(page: Page) {
  const items = [
    { id: GOBLIN, name: "Goblin Warrior", cr: "1/4", type: "fey", size: ["small"], ac: 15, hp: 10, source: "SRD 5.2.1 Monsters", ruleset: "dnd2024", legendary: false, kind: "monster" },
    { id: OWLBEAR, name: "Owlbear", cr: "3", type: "monstrosity", size: ["large"], ac: 13, hp: 59, source: "SRD 5.2.1 Monsters", ruleset: "dnd2024", legendary: false, kind: "monster" },
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

/** A DM with a party and an encounter of three goblins. */
async function prepare(request: APIRequestContext, testInfo: TestInfo) {
  const dm = await login(request, "admin", ACCOUNTS.admin.password);
  await api(request, dm.token, "POST", "/api/me/roles/dm", {});
  const partyName = `Tab ${testInfo.project.name} ${Math.random().toString(36).slice(2, 7)}`;
  const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: partyName });
  const enc = await api<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "Bridge ambush", publicName: "The bridge" });
  await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: GOBLIN, count: 3 });
  return { dm, party, partyName, enc };
}

const box = async (page: Page, selector: string) => (await page.locator(selector).first().boundingBox())!;

/** The list's heading row: ENCOUNTERS left and New encounter right on one line; no Assistant since 0.3.3. */
async function headingRow(page: Page, testInfo: TestInfo) {
  const head = page.locator(".enc-list-card .card-head").first();
  const title = await head.locator("h2").boundingBox();
  const add = await head.getByRole("button", { name: "New encounter" }).boundingBox();
  expect(title && add, "the heading row has ENCOUNTERS and New encounter").toBeTruthy();
  await expect(page.getByRole("button", { name: "Assistant" })).toHaveCount(0);
  const apart = (p: { x: number; y: number; width: number; height: number }, q: typeof p) =>
    p.x + p.width <= q.x + 0.5 || q.x + q.width <= p.x + 0.5 || p.y + p.height <= q.y + 0.5 || q.y + q.height <= p.y + 0.5;
  expect(apart(title!, add!), "New encounter doesn't sit on the heading").toBe(true);
  // One line: the heading on the left, the button on the right, no second row (0.3.3).
  const mid = (b: { y: number; height: number }) => b.y + b.height / 2;
  expect(Math.abs(mid(add!) - mid(title!)), `New encounter is on the heading's line (${testInfo.project.name})`).toBeLessThanOrEqual(4);
  expect(add!.x, "the button is right of the heading").toBeGreaterThan(title!.x + title!.width);
  // Nothing of the list's card sits above its own heading row.
  const card = await page.locator(".enc-list-card").first().boundingBox();
  expect(Math.min(title!.y, add!.y)).toBeGreaterThanOrEqual(card!.y - 0.5);
}

test.describe("Encounters and Combat tabs (0.3.1)", () => {
  test("a big screen: the whole width for session prep, then running it from the Combat tab", async ({ page, request }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop-wide", "the big-screen layout: 1440x900");
    const { dm, party, partyName, enc } = await prepare(request, testInfo);
    try {
      await seed(page, dm);
      await bestiary(page);
      await page.goto("/#/characters");
      const main = page.getByRole("navigation", { name: "Main" });
      const tab = main.getByRole("link", { name: "Encounters" });
      await expect(tab).toBeVisible({ timeout: 30_000 });
      await expect(main.getByRole("link", { name: "Combat" })).toBeVisible();
      await tab.click();
      // The parties are sub-tabs in the band under the main tabs (admin runs other specs' parties too); a party's
      // encounters are on the left.
      await expect(page).toHaveURL(/#\/encounters\/[^/]+$/);
      const partyTab = page.locator(".aur-subband").getByRole("tab", { name: partyName });
      await partyTab.click();
      await expect(page).toHaveURL(new RegExp(`#/encounters/${party.id}$`));
      await expect(partyTab).toHaveAttribute("aria-selected", "true");
      await expect(tab).toHaveAttribute("aria-current", "page");
      const list = page.getByRole("navigation", { name: `${partyName}: encounters` });
      await expect(page.getByText("Pick an encounter")).toBeVisible();
      await headingRow(page, testInfo);
      await list.getByRole("link", { name: "Open Bridge ambush" }).click();
      await expect(page).toHaveURL(new RegExp(`#/encounters/${party.id}/${enc.id}$`));
      await expect(page.getByRole("heading", { name: "Bridge ambush" })).toBeVisible();
      await expect(list.getByRole("link", { name: "Open Bridge ambush" })).toHaveAttribute("aria-current", "page");

      // Columns: the list, the creatures (the widest), then players and setup. No bottom bar.
      const view = page.viewportSize()!;
      const ws = await box(page, ".enc-ws");
      expect(ws.width, "the workspace uses the whole width").toBeGreaterThanOrEqual(view.width - 12);
      const left = await box(page, ".enc-ws-list");
      const creatures = await box(page, ".enc-creatures-card");
      const side = await box(page, ".enc-col-side");
      expect(left.x + left.width).toBeLessThanOrEqual(creatures.x + 1);
      expect(creatures.x + creatures.width).toBeLessThanOrEqual(side.x + 1);
      expect(creatures.width, "the creatures are the big panel").toBeGreaterThan(side.width * 1.5);
      await expect(page.locator(".enc-col-side")).toContainText("Players and combat");
      await expect(page.locator(".enc-col-side")).toContainText("Setup");
      await expect(page.getByRole("status", { name: "Encounter summary" })).toHaveCount(0);
      await expect(page.locator(".enc-panels")).toContainText("Before the fight");
      await expect(page.locator(".enc-panels")).toContainText("Encounter rules");
      expect((await measureOverflow(page)).page, "the page scrolls sideways").toBeLessThanOrEqual(0);
      await shot(page, testInfo, "builder");

      // "/" opens the picker full width under the creatures (0.3.5), in the page, with the focus in its search; Enter
      // adds and it stays open.
      await page.locator(".enc-creatures-card h2").click();
      await page.keyboard.press("/");
      const picker = page.getByRole("complementary", { name: "Add creatures" });
      await expect(picker).toBeVisible();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      const search = picker.getByLabel("Search creatures");
      await expect(search).toBeFocused();
      const docked = (await picker.boundingBox())!;
      const creatures2 = await box(page, ".enc-creatures-card");
      expect(docked.y, "the picker sits under the creatures").toBeGreaterThanOrEqual(creatures2.y + creatures2.height - 1);
      expect(Math.abs(docked.width - creatures2.width), "as wide as the creatures card").toBeLessThanOrEqual(2);
      const panels = await box(page, ".enc-panels");
      expect(panels.y, "the other cards stay below it").toBeGreaterThanOrEqual(docked.y + docked.height - 1);
      expect(docked.height, "its height is bounded").toBeLessThanOrEqual(page.viewportSize()!.height);
      const searchBox = (await search.boundingBox())!;
      const howMany = (await picker.getByLabel("How many to add").boundingBox())!;
      expect(Math.abs(searchBox.y + searchBox.height - (howMany.y + howMany.height)), "search and How many on one line").toBeLessThanOrEqual(4);
      const chips = await picker.locator(".enc-chips").evaluateAll((els) => els.map((e) => e.scrollWidth - e.clientWidth));
      expect(Math.max(...chips), "no sideways scroll in the filters").toBeLessThanOrEqual(0);
      const cols = await picker.locator(".enc-results").evaluate((el) => getComputedStyle(el).gridTemplateColumns.split(" ").length);
      expect(cols, "results in a grid of 2 to 4 columns").toBeGreaterThanOrEqual(2);
      expect(cols).toBeLessThanOrEqual(4);
      const heights = await picker.locator(".enc-result").evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().height)));
      expect(new Set(heights).size, "equal-height result cards").toBe(1);
      await search.fill("owl");
      await expect(picker.getByRole("button", { name: "Add Goblin Warrior" })).toHaveCount(0);
      await picker.getByLabel("How many to add").fill("2");
      await search.press("Enter");
      await expect(picker.getByText("Added 2 × Owlbear.")).toBeVisible();
      await expect(page.getByRole("heading", { name: "Creatures (5)" })).toBeVisible();
      await expect(search).toBeFocused();
      await expect(picker.getByLabel("How many to add"), "back to one after an add").toHaveValue("1");
      await search.fill("");
      await picker.getByRole("button", { name: "Add Goblin Warrior" }).click();
      await expect(page.getByRole("heading", { name: "Creatures (6)" })).toBeVisible();
      await expect(page.locator("main")).not.toContainText(/\bXP\b|difficulty/i);
      await shot(page, testInfo, "picker");
      await search.press("Escape");
      await expect(picker).toHaveCount(0);

      // Ready, then run it from the Combat tab.
      await page.getByRole("radiogroup", { name: "Status" }).getByRole("radio", { name: "Ready" }).click();
      await expect(page.getByRole("radiogroup", { name: "Status" }).getByRole("radio", { name: "Ready" })).toHaveAttribute("aria-checked", "true");
      await main.getByRole("link", { name: "Combat" }).click();
      await expect(page).toHaveURL(/#\/combat$/);
      const ready = page.getByRole("region", { name: "Ready to run" });
      const row = ready.getByRole("listitem", { name: "Bridge ambush" });
      await expect(row).toBeVisible({ timeout: 15_000 });
      await shot(page, testInfo, "combat");
      await row.getByRole("button", { name: "Start combat" }).click();
      await expect(page).toHaveURL(/#\/combat\/[^/]+$/);
      await expect(page.locator(".cb-page")).toBeVisible({ timeout: 15_000 });
      await expect(main.getByRole("link", { name: "Combat" })).toHaveAttribute("aria-current", "page");
      const tracker = await box(page, ".cb-page");
      expect(tracker.width, "the tracker uses the whole width").toBeGreaterThanOrEqual(view.width - 12);
      // No separate Summon: Add creature does that (summon spells come in 0.3.2).
      await expect(page.getByRole("button", { name: /^Summon/ })).toHaveCount(0);
      await shot(page, testInfo, "tracker");

      // Back on the Combat tab it is live, first.
      await main.getByRole("link", { name: "Combat" }).click();
      await expect(page.getByRole("region", { name: "Live now" }).getByRole("link", { name: "Resume Bridge ambush" })).toBeVisible({ timeout: 15_000 });
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });

  test("an account without the DM switch has neither tab, and the routes send it away", async ({ page, request }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop-wide", "one browser is enough");
    const other = SEEDED.find((s) => s.name === "cr-android-landscape")!;
    const session = await login(request, other.name, other.password);
    await seed(page, session);
    await page.goto("/#/characters");
    const main = page.getByRole("navigation", { name: "Main" });
    await expect(main.getByRole("link", { name: "Start" })).toBeVisible({ timeout: 30_000 });
    await expect(main.getByRole("link", { name: "Encounters" })).toHaveCount(0);
    await expect(main.getByRole("link", { name: "Combat" })).toHaveCount(0);
    for (const route of ["/encounters", "/combat"]) {
      await page.goto(`/#${route}`);
      await expect(page).not.toHaveURL(new RegExp(`#${route}`), { timeout: 15_000 });
    }
  });

  test("the phone: both tabs are rows on More, and the builder keeps its stacked layout", async ({ page, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "the user's phone: 384x832");
    const { dm, party, enc } = await prepare(request, testInfo);
    try {
      await seed(page, dm);
      await bestiary(page);
      await open(page, "/more");
      const more = page.getByRole("list", { name: "Account" });
      await expect(more.getByRole("link", { name: "Combat" })).toBeVisible({ timeout: 30_000 });
      await more.getByRole("link", { name: "Encounters" }).tap();
      // The last party opened here, or the first: admin runs other specs' parties too, so open this one.
      await expect(page).toHaveURL(/#\/encounters\/[^/]+$/);
      await open(page, `/encounters/${party.id}`);
      const row = page.getByRole("link", { name: "Open Bridge ambush" });
      await expect(row).toBeVisible();
      await headingRow(page, testInfo);
      await shot(page, testInfo, "list");
      await row.tap();
      await expect(page).toHaveURL(new RegExp(`#/encounters/${party.id}/${enc.id}$`));
      await expect(page.getByRole("link", { name: "Back to encounters" })).toBeVisible();
      await expect(page.getByRole("status", { name: "Encounter summary" })).toContainText("3 creatures");
      await expect(page.getByRole("complementary", { name: "Add creatures" })).toHaveCount(0);
      const o = await measureOverflow(page);
      expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
      const small = (await smallTargets(page, minTap(testInfo), ".enc-players")).map((t) => `${t.label} ${t.width}x${t.height}`);
      expect(small, "small controls in Players and combat").toEqual([]);
      await shot(page, testInfo, "builder");
      // The picker is the full-screen sheet here.
      await page.getByRole("region", { name: "Creatures" }).getByRole("button", { name: /Add creatures/ }).tap();
      const sheet = page.getByRole("dialog", { name: "Add creatures" });
      await settled(sheet);
      await expect(sheet.getByRole("button", { name: "Pick Owlbear" })).toBeVisible();
      await sheet.getByRole("button", { name: "Cancel" }).tap();

      await open(page, "/combat");
      await expect(page.getByRole("region", { name: "Live now" })).toBeVisible({ timeout: 30_000 });
      expect((await measureOverflow(page)).page).toBeLessThanOrEqual(0);
      await shot(page, testInfo, "combat");
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
