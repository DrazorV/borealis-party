import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page, Route, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, settled, signedInShell, smallTargets, test } from "./fixtures";

/**
 * The player's Combat sub-tab (0.3.0 part F) on the three phones (384x832, 832x384, 390x844). The party and the
 * player's character are real (this run's server); the combat is the server's PLAYER projection, answered here at
 * `/api/parties/{p}/combat` and `/api/combats/{c}/{action}` so the layout and the player's own writes are checked
 * whether or not the server under test has combat. Applying the DM's damage is the player's own `take_damage` on
 * the real server (D7 a). combat-player-live.spec.ts covers the real combat server with two accounts.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");

async function login(request: APIRequestContext, who: AccountName): Promise<{ token: string }> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as { token: string };
}

async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

async function newCharacter(request: APIRequestContext, auth: string): Promise<{ id: string; name: string }> {
  const report = await api<{ summary: { id: string; name: string } }>(request, auth, "POST", "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
  return { id: report.summary.id, name: report.summary.name };
}

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-combat-${name}.png`) });
}

/**
 * "Your turn" away from the Combat sub-tab: the banner under the title bar, or, on a phone held sideways (no row to
 * spare), the title bar's chip.
 */
function yourTurn(page: Page) {
  return page.viewportSize()!.height <= 480
    ? page.getByRole("button", { name: /^Your turn$/ })
    : page.getByRole("button", { name: /Your turn/ }).filter({ hasText: "Open combat" });
}

type Row = Record<string, unknown> & { id: string };
interface Mine { id: string; characterId?: string; hp: { current: number; max: number; temp: number }; life: unknown; concentration: string | null;
  concentrationChecks: { dc: number }[]; prompts: Record<string, unknown>[] }
interface Combat { id: string; partyId: string; encounter: { id: string; publicName: string }; status: string; pseq: number; turn: Record<string, unknown>;
  showEnemyTurn: boolean; order: Row[]; me: Mine[] }

/** The player projection (party/docs/combat-api.md) the routes answer, and what the player's writes did to it. */
function projection(partyId: string, hero: { id: string; name: string }): Combat {
  return {
    id: "rig-combat", partyId, encounter: { id: "e1", publicName: "Road ambush" }, status: "setup", pseq: 1, turn: { kind: "not_started" }, showEnemyTurn: false,
    order: [
      { id: "cr1", kind: "creature", name: "Goblin Boss", band: "bloodied", life: "alive", conditions: ["frightened"], exhaustion: 0, hp: { current: 9, max: 21 }, ac: 17,
        statBlock: { size: ["small"], creature_type: "humanoid", tags: ["goblinoid"], alignment: "neutral evil", ac: [{ value: 17, source: "chain shirt, shield" }],
          hp: { average: 21, formula: "6d6" }, speed: { walk: 30 }, abilities: { str: 10, dex: 14, con: 10, int: 10, wis: 8, cha: 10 },
          skills: { stealth: 6 }, senses: { darkvision: 60, passive_perception: 9 }, languages: ["Common", "Goblin"], cr: "1",
          traits: [{ name: "Nimble Escape", text: "The goblin can take the Disengage or Hide action as a bonus action on each of its turns." }],
          actions: [{ name: "Multiattack", text: "The goblin makes two attacks with its scimitar." }, { name: "Scimitar", text: "Melee Weapon Attack: +4 to hit, reach 5 ft., one target. Hit: 5 (1d6 + 2) slashing damage." }] } },
      { id: "me", kind: "character", name: hero.name, mine: true, initiative: null, band: "unhurt", life: "alive", conditions: [], exhaustion: 0, concentration: null },
      { id: "ally", kind: "character", name: "Bram the Steadfast", mine: false, initiative: 8, band: "bloodied", life: "alive", conditions: ["poisoned", "prone"], exhaustion: 0, concentration: null },
    ],
    me: [{ id: "me", characterId: hero.id, hp: { current: 10, max: 10, temp: 0 }, life: "alive", concentration: null, concentrationChecks: [], prompts: [] }],
  };
}

/** Answers the combat routes from `state`, as the server would for this player. */
async function serveCombat(page: Page, state: { combat: Combat | null }) {
  await page.route(/\/api\/parties\/[^/]+\/combat$/, (route: Route) =>
    route.fulfill({ json: { combat: state.combat?.id ?? null } }));
  await page.route(/\/api\/combats\/[^/]+$/, (route: Route) =>
    (state.combat ? route.fulfill({ json: state.combat }) : route.fulfill({ status: 404, json: { code: "not_found", message: "There is no such combat here." } })));
  await page.route(/\/api\/combats\/[^/]+\/[a-z_]+$/, async (route: Route) => {
    const c = state.combat!;
    const action = /\/([a-z_]+)$/.exec(route.request().url())![1];
    const { args } = route.request().postDataJSON() as { requestId: string; args: {
      id?: string; target?: string; owner?: string; roll?: { kind: string; total?: number; modifier?: number }; promptId?: string;
      parts?: { amount: { total: number } }[]; combatant?: { public_name: string; hp: { current: number; max: number }; ac: number | null };
    } };
    const row = c.order.find((x) => x.id === args.id);
    const own = c.me.find((x) => x.id === args.id);
    const total = args.roll?.kind === "typed" ? args.roll.total! : 10 + (args.roll?.modifier ?? 0);
    if (action === "set_initiative" && row) row.initiative = total;
    if (action === "summon" && args.combatant) {
      const id = `s${c.order.length}`;
      const hp = { ...args.combatant.hp, temp: 0 };
      const at = c.order.findIndex((x) => x.id === args.owner);
      c.order.splice(at + 1, 0, { id, kind: "creature", name: args.combatant.public_name, mine: true, band: "unhurt", life: { state: "alive" }, conditions: [], exhaustion: 0,
        view: { owner_id: args.owner, hp, ac: args.combatant.ac } });
      c.me.push({ id, hp, life: { state: "alive" }, concentration: null, concentrationChecks: [], prompts: [], kind: "creature" } as Mine);
    }
    if (action === "damage") {
      const pet = c.me.find((x) => x.id === args.target);
      if (pet) pet.hp = { ...pet.hp, current: Math.max(0, pet.hp.current - (args.parts?.[0]?.amount.total ?? 0)) };
    }
    if (action === "resolve_prompt") for (const m of c.me) m.prompts = m.prompts.filter((p) => p.id !== args.promptId);
    if (action === "death_save" && own && typeof own.life === "object") {
      const s = (own.life as { dying: { successes: number; failures: number } }).dying;
      if (total >= 10) s.successes += 1; else s.failures += 1;
    }
    c.pseq += 1;
    await route.fulfill({ json: c });
  });
}

test.describe("The player's Combat sub-tab (0.3.0)", () => {
  test("initiative, the damage prompt, your turn and a stat block, on a phone", async ({ page, signIn, request }, testInfo) => {
    test.setTimeout(180_000);
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Fight ${Math.random().toString(36).slice(2, 8)}` });
    const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
    const hero = await newCharacter(request, player.token);
    await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero.id });
    const state = { combat: projection(party.id, hero) as Combat | null };
    await serveCombat(page, state);
    try {
      await signIn("player");
      // The Live chip shows on the Start page and opens the hero's Combat sub-tab.
      await open(page, "/characters");
      const live = page.getByRole("button", { name: /^Live$/ });
      await expect(live).toBeVisible({ timeout: 15_000 });
      await live.tap();
      const pane = page.getByTestId("combat-pane");
      await expect(pane).toBeVisible();
      await expect(page.getByRole("tab", { name: "Combat" })).toHaveAttribute("aria-selected", "true");
      await expect(pane.getByText("Setup")).toBeVisible();

      // Initiative: both ways to roll, then typed from the table.
      const init = pane.getByRole("alertdialog", { name: "Roll initiative" });
      await expect(init.getByRole("button", { name: "Roll in app" })).toBeVisible();
      await fits(page, testInfo, ".combat-pane");
      await shot(page, testInfo, "initiative");
      await init.getByRole("button", { name: "Type my table roll" }).tap();
      await init.getByLabel("Initiative total").fill("14");
      await init.getByRole("button", { name: "Send" }).tap();
      await expect(init).toHaveCount(0);
      await expect(pane.getByLabel("Initiative 14")).toBeVisible();

      // Active, with the DM's damage: Apply takes it on the player's own character on the server.
      const hpLabel = pane.getByRole("status", { name: /^Hit points: \d+ of \d+/ });
      const [, before] = /Hit points: (\d+)/.exec((await hpLabel.getAttribute("aria-label"))!)!.map(Number) as [number, number];
      state.combat = { ...state.combat!, status: "active", turn: { kind: "combatant", id: "cr1", round: 1 } };
      state.combat.me[0]!.prompts = [{ id: "pr1", kind: "damage", amount: 3, parts: [{ amount: 3, damage_type: "slashing" }], criticalHit: false, at: 1 }];
      // A reload opens no character in a browser: the Live chip opens the one in the fight.
      await page.reload();
      await expect(page.locator("html.phone")).toHaveCount(1);
      await page.getByRole("button", { name: /^Live$/ }).tap();
      const prompt = page.getByRole("alertdialog", { name: "Take 3 slashing damage?" });
      await expect(prompt).toBeVisible();
      await expect(pane.getByText("Round 1")).toBeVisible();
      await fits(page, testInfo, ".combat-pane");
      await shot(page, testInfo, "damage-prompt");
      await prompt.getByRole("button", { name: "Apply" }).tap();
      await expect(prompt).toHaveCount(0);
      await expect(pane.getByRole("status", { name: new RegExp(`^Hit points: ${Math.max(0, before - 3)} of`) })).toBeVisible();
      const saved = await api<{ play: { hitPoints: { current: number } } }>(request, player.token, "POST", "/api/rpc/get_snapshot", { characterId: hero.id });
      expect(saved.play.hitPoints.current, "the player's own character took it").toBe(Math.max(0, before - 3));

      // The turn order: visible combatants only, the one acting marked, allies' bands and conditions.
      const order = pane.getByRole("list", { name: "Turn order" });
      await expect(order.locator(".combat-row")).toHaveCount(3);
      await expect(order.locator('.combat-row[aria-current="true"]')).toContainText("Goblin Boss");
      await expect(order.locator(".combat-row").nth(2).getByLabel("Health: Bloodied")).toBeVisible();

      // The revealed creature's stat block, as a sheet.
      await pane.getByRole("button", { name: "Goblin Boss stat block" }).tap();
      const sheet = page.getByRole("dialog");
      await settled(sheet);
      await expect(sheet.getByText("Armor Class")).toBeVisible();
      await expect(sheet.getByText("Nimble Escape.")).toBeVisible();
      const o = await measureOverflow(page);
      expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
      await shot(page, testInfo, "stat-block");
      await page.keyboard.press("Escape");
      await expect(sheet).toHaveCount(0);

      // Your turn: the banner on another page, and the Live chip says it too.
      state.combat = { ...state.combat!, turn: { kind: "combatant", id: "me", round: 1 } };
      await open(page, "/characters");
      await page.reload();
      const banner = yourTurn(page);
      await expect(banner).toBeVisible({ timeout: 15_000 });
      // One row: the banner is the Live chip widened, and the page stays in view (a phone held sideways too).
      await expect(page.getByRole("button", { name: /^Live$/ })).toHaveCount(0);
      await expect(page.locator("main").first()).toBeInViewport();
      await fits(page, testInfo, ".party-bar-host");
      await shot(page, testInfo, "your-turn-banner");
      await banner.tap();
      await expect(page.getByTestId("combat-pane").locator(".combat-now.yours")).toContainText("Your turn");
      await expect(page.getByRole("button", { name: /^Your turn$/ })).toBeVisible();

      // The combat ends: the sub-tab and the chip go.
      state.combat = null;
      const asked = page.waitForResponse(/\/api\/parties\/[^/]+\/combat$/);
      await page.reload();
      await asked;
      await expect(page.locator("html.phone")).toHaveCount(1);
      await signedInShell(page);
      // The browser may or may not open the character again after a reload; either way there is no Combat sub-tab.
      await expect(page.getByText("No character open").or(page.getByRole("tab", { name: "Abilities" }))).toBeVisible({ timeout: 15_000 });
      await expect(page.getByRole("tab", { name: "Combat" })).toHaveCount(0);
      await expect(page.getByRole("button", { name: /^Live$/ })).toHaveCount(0);
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });

  test("dying: death saves rolled in the app or typed", async ({ page, signIn, request }, testInfo) => {
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Saves ${Math.random().toString(36).slice(2, 8)}` });
    const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
    const hero = await newCharacter(request, player.token);
    await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero.id });
    const combat = projection(party.id, hero);
    combat.status = "active";
    combat.turn = { kind: "combatant", id: "me", round: 3 };
    Object.assign(combat.order[1]!, { initiative: 12, conditions: ["unconscious"] });
    Object.assign(combat.me[0]!, { hp: { current: 0, max: 10, temp: 0 }, life: { dying: { successes: 1, failures: 1 } } });
    await serveCombat(page, { combat });
    try {
      await signIn("player");
      await open(page, "/characters");
      await yourTurn(page).tap();
      const saves = page.getByRole("group", { name: "Death saves" });
      await expect(saves).toBeVisible();
      await expect(saves.getByRole("img", { name: "Successes: 1 of 3" })).toBeVisible();
      await expect(saves.getByRole("img", { name: "Failures: 1 of 3" })).toBeVisible();
      await saves.scrollIntoViewIfNeeded();
      await fits(page, testInfo, ".combat-pane");
      await shot(page, testInfo, "death-saves");
      await saves.getByRole("button", { name: "Type my table roll" }).tap();
      await saves.getByLabel("Death save total").fill("4");
      await saves.getByRole("button", { name: "Send" }).tap();
      await expect(saves.getByRole("img", { name: "Failures: 2 of 3" })).toBeVisible();
      await saves.getByRole("button", { name: "Roll in app" }).tap();
      await expect(saves.getByRole("img", { name: "Successes: 2 of 3" })).toBeVisible();
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });

  test("summon: a quick stat block follows the PC in the turn order, and the player hurts it", async ({ page, signIn, request }, testInfo) => {
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Summons ${Math.random().toString(36).slice(2, 8)}` });
    const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
    const hero = await newCharacter(request, player.token);
    await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero.id });
    const combat = projection(party.id, hero);
    combat.status = "active";
    combat.turn = { kind: "combatant", id: "cr1", round: 2 };
    combat.order[1]!.initiative = 12;
    const state = { combat: combat as Combat | null };
    await serveCombat(page, state);
    try {
      await signIn("player");
      await open(page, "/characters");
      await page.getByRole("button", { name: /^Live$/ }).tap({ timeout: 15_000 });
      const pane = page.getByTestId("combat-pane");
      await pane.getByRole("button", { name: "Summon…" }).tap();
      const dialog = page.getByRole("dialog", { name: "Summon" });
      await settled(dialog);
      await dialog.getByLabel("Name").fill("Spirit Bear");
      await dialog.getByLabel("Hit points").fill("15");
      await dialog.getByLabel("Armor class").fill("13");
      const o1 = await measureOverflow(page);
      expect(o1.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
      await shot(page, testInfo, "summon-dialog");
      await dialog.getByRole("button", { name: "Summon", exact: true }).tap();
      await expect(dialog).toHaveCount(0);
      const rows = pane.getByRole("list", { name: "Turn order" }).locator(".combat-row .combat-row-name");
      await expect(rows).toHaveText([/^Goblin Boss/, `${hero.name} (you)`, "Spirit Bear (yours)", "Bram the Steadfast"]);
      const card = pane.getByRole("region", { name: "Your creatures" }).getByRole("listitem", { name: "Spirit Bear" });
      await card.getByLabel("Spirit Bear amount").fill("4");
      await card.getByRole("button", { name: "Damage Spirit Bear" }).tap();
      await expect(card.getByRole("status", { name: "Spirit Bear hit points: 11 of 15" })).toBeVisible();
      await card.scrollIntoViewIfNeeded();
      await fits(page, testInfo, ".combat-pane");
      await shot(page, testInfo, "summon-card");
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
