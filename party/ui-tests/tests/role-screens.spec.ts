import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { APIRequestContext, Browser, Page } from "@playwright/test";
import { devices } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, test } from "./fixtures";

/**
 * The role screens (0.3.3, user 2026-11-14) against the REAL server, with two browsers: the DM on a 1440x900 desktop
 * in the Combat tab (the DM screen), the player on a 384x832 phone in the Play tab (the player screen).
 * - The player casts Bless from their own turn panel: the DM sees its effect chips, never the PC's HP.
 * - The DM sends damage to the player: the player accepts it on their Play tab; the DM sees it accepted.
 * Skipped on a server without player casts (party/docs/role-views.md, part F of 0.3.3).
 * Screenshots go to PARTY_SHOTS when it is set.
 */

const PHONE = {
  viewport: { width: 384, height: 832 }, deviceScaleFactor: 2.8125, isMobile: true, hasTouch: true,
  userAgent: "Mozilla/5.0 (Linux; Android 16; SM-S948B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36",
};
const DESK = { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } };

interface Session { token: string; user: unknown }
async function login(request: APIRequestContext, who: AccountName): Promise<Session> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as Session;
}
async function call(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown) {
  return request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
}
async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await call(request, auth, method, path, data);
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}
let n = 0;
const rid = () => `roles-${Date.now().toString(36)}-${n++}`;

interface Slot { id: string; title: string; elementType: string }
interface Snapshot { id: string; name: string; slots: Slot[]; progress: { nextSlotId: string | null }; spellcasting: { classes: { classSlotId: string; spells: { id: string; name: string; prepared: boolean }[] }[] } }

/** A level 1 SRD cleric with Bless prepared, built through the server as the builder would. */
async function cleric(request: APIRequestContext, auth: string, name: string): Promise<Snapshot> {
  const rpc = <T,>(command: string, data: object) => api<T>(request, auth, "POST", `/api/rpc/${command}`, data);
  let s = await rpc<Snapshot>("create_character", { request: { name, ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} } });
  for (let i = 0; i < 40; i++) {
    const next = s.slots.find((x) => x.id === s.progress.nextSlotId);
    if (!next) break;
    const options = await rpc<{ items: { id: string; name?: string; disabledReason?: string }[] }>("get_slot_options", { characterId: s.id, slotId: next.id, limit: 80 });
    const pick = next.elementType === "class" ? options.items.find((o) => /CLERIC/i.test(o.id)) : options.items.find((o) => !o.disabledReason);
    if (!pick) break;
    s = await rpc<Snapshot>("apply_selection", { characterId: s.id, slotId: next.id, elementId: pick.id });
  }
  const block = s.spellcasting.classes[0];
  expect(block, "the cleric casts spells").toBeTruthy();
  const bless = block!.spells.find((x) => x.name === "Bless");
  expect(bless, "Bless is on the cleric's list").toBeTruthy();
  if (!bless!.prepared) s = await rpc<Snapshot>("set_spell_prepared", { characterId: s.id, classSlotId: block!.classSlotId, spellId: bless!.id, prepared: true });
  await rpc("save_character", { characterId: s.id });
  return s;
}

async function context(browser: Browser, baseURL: string, session: Session, opts: object): Promise<Page> {
  const ctx = await browser.newContext({ baseURL, ...opts });
  await ctx.addInitScript((s) => { localStorage.setItem("party.session", s); }, JSON.stringify({ baseUrl: "", ...session }));
  return ctx.newPage();
}

async function shot(page: Page, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `role-screens-${name}.png`) });
}

interface DmView { id: string; state: { combatants: { id: string; kind: string; label: string }[] } }

test.describe("Role screens: the DM's Combat tab and the player's Play tab (0.3.3)", () => {
  test("the DM sends damage and the player accepts it; the player casts Bless and the DM sees its effect chips but no HP", async ({ request, browser, baseURL }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop-wide", "two browsers: the DM at 1440x900, the player at 384x832");
    test.setTimeout(240_000);
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Roles ${Math.random().toString(36).slice(2, 8)}` });
    try {
      const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
      const hero = await cleric(request, player.token, `Aria ${Math.random().toString(36).slice(2, 6)}`);
      await api(request, player.token, "POST", "/api/rpc/load_character", { characterId: hero.id });
      await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero.id });

      const enc = await api<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "Roles rig", publicName: "Road ambush" });
      await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: "ID_SRD521_MONSTER_GOBLIN_WARRIOR", hidden: false, publicName: "Goblin" });
      const started = await api<DmView>(request, dm.token, "POST", `/api/encounters/${enc.id}/start`, { requestId: rid() });
      const pc = started.state.combatants.find((c) => c.kind === "character")!;
      const goblin = started.state.combatants.find((c) => c.kind === "creature")!;
      // The PC first, so it's the player's turn: the DM types both table rolls and starts.
      await api(request, dm.token, "POST", `/api/combats/${started.id}/set_initiative`, { requestId: rid(), args: { id: goblin.id, roll: { kind: "typed", total: 3 } } });

      const phone = await context(browser, baseURL!, player, PHONE);
      const desk = await context(browser, baseURL!, dm, DESK);

      // The player: the live combat is the top of the Play tab (no Combat sub-tab), initiative typed first.
      await phone.goto("/?mock=mobile#/characters");
      await phone.getByRole("button", { name: /^Live$/ }).tap({ timeout: 30_000 });
      const pane = phone.getByTestId("combat-pane");
      await expect(pane).toBeVisible();
      await expect(phone.getByRole("tab", { name: "Combat" })).toHaveCount(0);
      const init = pane.getByRole("alertdialog", { name: "Roll initiative" });
      await init.getByLabel("Initiative: my table roll").fill("19");
      await init.getByRole("button", { name: "Send" }).tap();
      await expect(pane.getByLabel("Initiative 19")).toBeVisible();
      await api(request, dm.token, "POST", `/api/combats/${started.id}/start`, { requestId: rid(), args: {} });
      await expect(pane.locator(".combat-now")).toContainText("Your turn", { timeout: 10_000 });
      // Rests on the tiles' row; nothing scrolls sideways at 384 px.
      await expect(phone.getByRole("group", { name: "Rests" })).toBeVisible();
      expect((await measureOverflow(phone)).page).toBeLessThanOrEqual(0);
      await shot(phone, "player-turn");
      await phone.getByRole("group", { name: "Rests" }).scrollIntoViewIfNeeded();
      await shot(phone, "player-rests");

      // The DM's Combat tab: the PC's row and card show no HP of the PC anywhere.
      await desk.goto(`/#/combat/${started.id}`);
      const row = desk.locator(`[data-combatant="${pc.id}"]`);
      await expect(row).toBeVisible({ timeout: 30_000 });
      await expect(row.locator(".cb-row-hp")).toHaveCount(0);
      await expect(row.getByLabel(/hit points/)).toHaveCount(0);
      const sheetHp = (await api<{ play: { hitPoints: { current: number; max: number } } }>(request, player.token, "POST", "/api/rpc/get_snapshot", { characterId: hero.id })).play.hitPoints;
      await row.locator(".cb-row-main").click();
      const card = desk.getByRole("region", { name: new RegExp(`${hero.name} details`) });
      await expect(card).toBeVisible();
      await expect(card.getByText(`${sheetHp.current}/${sheetHp.max}`)).toHaveCount(0);
      await expect(card.getByLabel(/hit points/)).toHaveCount(0);
      await shot(desk, "dm-screen");

      // The DM sends 4 slashing to the player; the player accepts it; the DM sees it accepted, never the HP.
      await card.getByLabel(/^Amount for /).fill("4");
      await card.getByLabel("Damage type").selectOption("slashing");
      await card.getByRole("button", { name: "Send damage" }).click();
      const sent = card.getByRole("list", { name: /^Sent to / });
      await expect(sent).toContainText("4 slashing damage");
      await expect(sent).toContainText("Delivered");
      await shot(desk, "dm-sent");
      const prompt = pane.getByRole("alertdialog", { name: /Take 4 slashing damage\?/ });
      await expect(prompt).toBeVisible({ timeout: 10_000 });
      await shot(phone, "player-damage-prompt");
      await prompt.getByRole("button", { name: "Apply" }).tap();
      await expect(prompt).toHaveCount(0);
      const after = (await api<{ play: { hitPoints: { current: number } } }>(request, player.token, "POST", "/api/rpc/get_snapshot", { characterId: hero.id })).play.hitPoints.current;
      expect(after).toBe(Math.max(0, sheetHp.current - 4));
      await expect(sent).toContainText(/Accepted|Answered/, { timeout: 10_000 });
      await expect(card.getByText(new RegExp(`\\b${after}/${sheetHp.max}\\b`))).toHaveCount(0);
      await shot(desk, "dm-accepted");
      // The player's Play tab on a desktop: the rests sit on the stat tiles' row.
      const wide = await context(browser, baseURL!, player, DESK);
      await wide.goto("/#/characters");
      await wide.getByRole("button", { name: /Your turn/ }).first().click({ timeout: 30_000 });
      const rests = wide.getByRole("group", { name: "Rests" });
      await expect(rests).toBeVisible({ timeout: 30_000 });
      const tile = await wide.locator(".bp-stat-row .stat").first().boundingBox();
      const restBox = await rests.boundingBox();
      expect(Math.abs((restBox?.y ?? 0) - (tile?.y ?? 999)), "the rests are on the tiles' row").toBeLessThan(12);
      await rests.scrollIntoViewIfNeeded();
      await shot(wide, "player-desktop-rests");
      await wide.context().close();

      // The player casts Bless on themself from their own turn panel.
      const panel = pane.getByRole("region", { name: "Your turn panel" });
      await panel.getByRole("button", { name: /^Bless L1/ }).tap();
      const sheet = phone.getByRole("dialog", { name: "Cast Bless" });
      await sheet.getByRole("button", { name: /\(you\)$/ }).tap();
      await shot(phone, "player-cast-sheet");
      const cast = phone.waitForResponse((r) => r.url().includes(`/api/combats/${started.id}/cast_spell`));
      await sheet.getByRole("button", { name: "Cast Bless" }).tap();
      const answer = await cast;
      test.skip(answer.status() === 404, "this server takes no player casts yet (party/docs/role-views.md)");
      expect(answer.ok(), `cast_spell: ${answer.status()} ${await answer.text()}`).toBeTruthy();
      await expect(sheet).toHaveCount(0);

      // The DM sees Bless as an effect chip on the PC (and a toast), still no HP.
      await expect(row.locator(".cb-fx-chip", { hasText: "Bless" })).toBeVisible({ timeout: 10_000 });
      await expect(desk.getByText(`${after}/${sheetHp.max}`)).toHaveCount(0);
      await shot(desk, "dm-bless-chips");
      await shot(phone, "player-after");
      await phone.context().close();
      await desk.context().close();
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
