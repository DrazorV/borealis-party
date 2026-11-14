import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, finding, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * The spell-cast flow against the REAL server (0.3.3): spell-combat.spec.ts runs the same screens against the tracker's
 * mock, this one against the server's own engine, the bundled SRD 5.1 content and its curated spell rows. The DM
 * (`admin`) has a 2014 party with the player's PC (`player`, the d5-hero fixture) and an encounter with the SRD Mage
 * and an SRD Ogre. In the tracker: initiative typed, Mage Armor (the AC breakdown), Fireball at level 3 with typed
 * table rolls (the Ogre's HP, the slots), and Shield as the Mage's reaction on another creature's turn. On the S26
 * Ultra's 384x832 and on a DM's 1440x900.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const MAGE = "ID_SRD51_MONSTER_MAGE";
const OGRE = "ID_SRD51_MONSTER_OGRE";

interface Combatant {
  id: string; kind: string; label: string; ac: number; hp: { current: number; max: number };
  spell_slots?: Record<string, { max: number; remaining: number }>;
  economy: { action: boolean; bonus_action: boolean; reaction: boolean };
  abilities?: { key: string; name: string }[];
}
interface Effect { id: string; name: string; target?: string; targets?: string[] }
interface DmView { id: string; seq: number; state: { round: number; combatants: Combatant[]; effects: Effect[] | Record<string, Effect> } }

/** The names of the effects on `id` in the DM's view. */
function effectsOn(v: DmView, id: string): string[] {
  const all = Array.isArray(v.state.effects) ? v.state.effects : Object.values(v.state.effects);
  const on = all.filter((e) => e.target === id || e.targets?.includes(id) || JSON.stringify(e).includes(id)).map((e) => e.name);
  return on;
}

async function login(request: APIRequestContext, who: AccountName): Promise<{ token: string }> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as { token: string };
}

async function api<T>(request: APIRequestContext, token: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${token}` }, ...(data === undefined ? {} : { data }) });
  const text = await r.text();
  expect(r.ok(), `${method} ${path}: ${r.status()} ${text.slice(0, 400)}`).toBeTruthy();
  return (text ? JSON.parse(text) : null) as T;
}

/**
 * Starts the encounter. Under the full rig's parallel load a start has answered 409 "This party already has a live
 * combat" for a party with none before it; then the party's live combat (if the start made one after all) is used,
 * else the start is tried again, and the case is recorded as a finding.
 */
async function startCombat(request: APIRequestContext, token: string, party: string, encounter: string, testInfo: TestInfo): Promise<DmView> {
  for (let attempt = 1; ; attempt++) {
    const r = await request.fetch(`/api/encounters/${encounter}/start`, {
      method: "POST", headers: { authorization: `Bearer ${token}` }, data: { requestId: `rig-${randomUUID()}` } });
    if (r.ok()) return (await r.json()) as DmView;
    const text = await r.text();
    expect(r.status(), `start: ${r.status()} ${text.slice(0, 300)}`).toBe(409);
    const live = await api<{ combat: string | null }>(request, token, "GET", `/api/parties/${party}/combat`);
    finding(testInfo, `encounter start answered 409 (${text.slice(0, 120)}) on attempt ${attempt}; the party's live combat: ${live.combat ?? "none"}`);
    if (live.combat) return api<DmView>(request, token, "GET", `/api/combats/${live.combat}`);
    expect(attempt, "start keeps answering 409").toBeLessThan(3);
    await new Promise((ok) => setTimeout(ok, 500 * attempt));
  }
}

const phone = (testInfo: TestInfo) => testInfo.project.name === "android-portrait";

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-spells-live-${name}.png`), fullPage: false });
}

async function noSideways(page: Page) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
}

test.describe("Casting spells against the real server (0.3.3)", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(!["android-portrait", "desktop-wide"].includes(testInfo.project.name), "the S26 Ultra and a DM's 1440x900 screen");
  });

  test("Mage Armor, Fireball with typed rolls and Shield as a reaction: HP, slots and AC from the server", async ({ page, request, signIn }, testInfo) => {
    test.setTimeout(240_000);
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", {
      name: `Spells live ${testInfo.project.name} ${Math.random().toString(36).slice(2, 8)}`, ruleset: "dnd2014" });
    try {
      // The PC: the player's own character, joined with a code.
      const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
      const hero = (await api<{ summary: { id: string } }>(request, player.token, "POST", "/api/rpc/import_character", {
        fileName: "d5-hero.dnd5e", contentBase64: readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64") })).summary.id;
      await api(request, player.token, "POST", "/api/rpc/load_character", { characterId: hero });
      await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero });

      const enc = await api<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "Tower top" });
      await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: MAGE, count: 1, hidden: false });
      await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: OGRE, count: 1, hidden: false });
      const started = await startCombat(request, dm.token, party.id, enc.id, testInfo);
      const view = async () => api<DmView>(request, dm.token, "GET", `/api/combats/${started.id}`);
      const who = (v: DmView, label: string) => v.state.combatants.find((c) => c.label === label)!;
      const at = await view();
      const mage = who(at, "Mage");
      const ogre = who(at, "Ogre");
      const pc = at.state.combatants.find((c) => c.kind === "character")!;
      expect(mage.spell_slots?.["1"]).toEqual({ max: 4, remaining: 4 });
      expect(mage.spell_slots?.["3"]).toEqual({ max: 3, remaining: 3 });
      expect(mage.ac).toBe(12);
      // The server links the stat block's spells to the curated SRD rows: Fireball, Mage Armor and Shield.
      const key = (name: string) => mage.abilities!.find((a) => a.name.toLowerCase() === name)!.key;
      const fireball = key("fireball");
      const mageArmor = key("mage armor");

      // The DM's tracker, initiative typed from the table.
      await signIn("admin");
      if (phone(testInfo)) await open(page, `/combat/${started.id}`);
      else {
        await page.goto(`/#/combat/${started.id}`);
        await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
      }
      await signedInShell(page);
      await expect(page.locator("[data-combatant]").first()).toBeVisible({ timeout: 30_000 });
      for (const [label, n] of [["Mage", 18], [pc.label, 12], ["Ogre", 6]] as const) {
        const box = page.getByLabel(`${label} initiative`, { exact: true });
        await box.fill(String(n));
        await box.press("Enter");
      }
      const next = page.locator(".cb-thumb:visible .btn, .cb-top-actions .cb-next:visible").first();
      await next.click();
      await expect(page.getByText("ROUND 1").first()).toBeVisible();
      await expect(page.getByRole("heading", { name: "Mage's turn" })).toBeVisible();
      const card = page.getByRole("region", { name: "Mage details" });
      const panel = card.locator("[data-turn-panel]");

      // Mage Armor on itself: a slot spent, and the AC chip explains the new AC.
      await panel.locator(`[data-turn-button="${mageArmor}"]`).click();
      const armor = page.getByRole("dialog", { name: "Cast Mage Armor" });
      await expect(armor).toBeVisible();
      await armor.getByRole("button", { name: "Mage", exact: true }).click();
      await armor.getByRole("button", { name: "Cast", exact: true }).click();
      await expect(armor).toBeHidden();
      await expect(card.getByText("15 = Mage Armor 13 + DEX 2")).toBeVisible();
      // The server keeps the stat block's AC and adds Mage Armor as an effect on the Mage (the tracker works out 15).
      await expect.poll(async () => effectsOn(await view(), mage.id), { message: "Mage Armor's effect on the Mage" }).toContain("Mage Armor");
      let now = await view();
      expect(who(now, "Mage").spell_slots?.["1"]?.remaining).toBe(3);
      await shot(page, testInfo, "mage-armor");
      await noSideways(page);

      // The PC's turn, then the Ogre's. The Ogre attacks the Mage: the DM opens the Mage (its reaction is still
      // ready, so its panel has the reactions strip) and casts Shield from it.
      await next.click();
      await next.click();
      await expect(page.getByRole("heading", { name: "Ogre's turn" })).toBeVisible();
      await page.locator(`[data-combatant="${mage.id}"] .cb-row-main:visible`).first().click();
      const strip = card.getByRole("toolbar", { name: "Reactions ready" });
      await expect(strip).toContainText(/shield L1\+/i);
      await expect(strip).toContainText(/counterspell L3\+/i);
      // The buttons take the stat block's own words, which SRD 5.1 prints in lower case ("shield", "fire bolt").
      const lower = (await panel.locator("[data-turn-button^=spell_]").allTextContents()).filter((t) => /^[a-z]/.test(t.trim()));
      if (lower.length) finding(testInfo, `turn panel: ${lower.length} spell buttons in lower case, e.g. ${lower.slice(0, 3).map((t) => `"${t.trim()}"`).join(", ")}`);
      const shieldButton = strip.getByRole("button", { name: /^shield/i });
      await shieldButton.scrollIntoViewIfNeeded();
      await shieldButton.click();
      const shieldSheet = page.getByRole("dialog", { name: "Cast Shield" });
      await expect(shieldSheet).toBeVisible();
      await shieldSheet.getByRole("button", { name: "Cast", exact: true }).click();
      await expect(shieldSheet).toBeHidden();
      await expect.poll(async () => effectsOn(await view(), mage.id), { message: "Shield's effect on the Mage" }).toContain("Shield");
      await expect.poll(async () => who(await view(), "Mage").spell_slots?.["1"]?.remaining, { message: "Shield's slot" }).toBe(2);
      now = await view();
      expect(who(now, "Mage").economy.reaction, "the reaction is spent").toBe(true);
      expect(who(now, "Mage").spell_slots?.["1"]?.remaining).toBe(2);
      await shot(page, testInfo, "shield");

      // Round 2, the Mage's turn: Fireball at level 3 on the Ogre and the PC, every roll typed from the table.
      await next.click();
      await expect(page.getByRole("heading", { name: "Mage's turn" })).toBeVisible();
      await panel.locator(`[data-turn-button="${fireball}"]`).click();
      const sheet = page.getByRole("dialog", { name: "Cast Fireball" });
      await expect(sheet).toBeVisible();
      await sheet.getByRole("radio", { name: /L3/ }).click();
      await expect(sheet.locator("[data-preview]")).toContainText("8d6 fire");
      await sheet.getByRole("button", { name: "Ogre", exact: true }).click();
      await sheet.getByRole("button", { name: pc.label, exact: true }).click();
      await noSideways(page);
      await shot(page, testInfo, "cast-sheet");
      // The tracker shows the cast at once from its own prediction; the server's answer then brings the cast under the
      // server's id, and its rows may differ (see the ray finding below). Typing waits for that answer.
      const answered = page.waitForResponse((r) => r.request().method() === "POST" && r.url().endsWith("/cast_spell"));
      await sheet.getByRole("button", { name: "Cast", exact: true }).click();
      expect((await answered).ok(), "the server takes the cast").toBeTruthy();

      const res = page.getByRole("dialog", { name: "Fireball · Mage" });
      await expect(res).toBeVisible();
      // A save row's typed field: "<name>: DEX save: typed result", or "<name> (ray N): ..." when the row has a ray.
      const saveBox = (name: string) => res.getByLabel(new RegExp("^" + name + String.raw`( \(ray \d+\))?: DEX save: typed result$`));
      const rayed = await res.getByLabel(/\(ray \d+\): DEX save/).count();
      if (rayed) finding(testInfo, `Fireball resolution: ${rayed} save rows from the server are labelled "(ray N)" (a sphere has no rays), and the rows' keys change from the predicted cast to the server's, so a number typed before the answer is lost`);
      const ogreSave = saveBox("Ogre");
      await expect(ogreSave).toHaveAttribute("inputmode", "numeric");
      await ogreSave.fill("4");
      await ogreSave.press("Enter");
      await expect(res.locator(`[data-target="${ogre.id}"]`)).toContainText("Failed");
      const pcRow = res.locator(`[data-target="${pc.id}"]`);
      await expect(pcRow).toContainText(`Waiting for ${pc.label}'s player`);
      await pcRow.getByRole("button", { name: "Type it" }).click();
      const pcSave = saveBox(pc.label);
      await pcSave.fill("19");
      await pcSave.press("Enter");
      await expect(pcRow).toContainText("Saved");
      const damage = res.getByLabel("Fireball damage: typed total");
      await expect(damage).toBeEnabled();
      await damage.fill("28");
      await shot(page, testInfo, "resolution");
      await noSideways(page);
      await res.getByRole("button", { name: "Apply", exact: true }).click();
      await expect(res).toBeHidden();

      // The tracker sends Apply from its write queue: the server has it a moment later.
      await expect.poll(async () => who(await view(), "Ogre").hp.current, { message: "the Ogre failed: all 28" }).toBe(ogre.hp.current - 28);
      now = await view();
      expect(who(now, "Mage").spell_slots?.["3"]?.remaining, "a 3rd-level slot spent").toBe(2);
      await expect(page.locator(`[data-combatant="${ogre.id}"]`)).toContainText(String(ogre.hp.current - 28));
      // The PC saved: half, 14 fire. Its HP lives on the player's sheet, so the player's projection gets a damage prompt.
      await expect.poll(async () => {
        const v = await api<{ me: { id: string; prompts: { kind: string; amount: number; parts: { amount: number; damage_type?: string }[] }[] }[] }>(
          request, player.token, "GET", `/api/combats/${started.id}`);
        return (v.me.find((m) => m.id === pc.id)?.prompts ?? []).map((p) => `${p.kind} ${p.amount} ${p.parts.map((x) => x.damage_type).join("+")}`);
      }, { message: "the player's damage prompt" }).toContain("damage 14 fire");
      await noSideways(page);
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
