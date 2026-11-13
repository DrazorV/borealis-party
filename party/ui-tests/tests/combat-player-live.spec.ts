import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Browser, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, test } from "./fixtures";

/**
 * The player's Combat sub-tab against the REAL combat server (0.3.0, party/docs/combat-api.md), with two accounts:
 * `admin` is the DM and runs the fight through the API, `player` plays on a phone. The player sees a reveal within
 * about two seconds, and a hidden creature is never in the page or in any answer the player's phone gets (plan 3.5,
 * D5). Skipped on a server without the combat routes (any build before feat/combat-server is merged).
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
const LIVE_MS = 2_500;
/** "Constant updating": a DM change the player can see shows on the phone within a second, with no reload. */
const QUICK_MS = 1_000;
/** What only the DM knows about the hidden creature; none of it may reach the player. */
const HIDDEN_LABEL = "Ambusher-DM-ONLY";
const HIDDEN_PUBLIC = "Lurker";

async function login(request: APIRequestContext, who: AccountName): Promise<{ token: string; user: unknown }> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as { token: string; user: unknown };
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

let request_ = 0;
const rid = () => `rig-${Date.now().toString(36)}-${request_++}`;

async function phone(browser: Browser, testInfo: TestInfo, baseURL: string, session: { token: string; user: unknown }): Promise<Page> {
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = testInfo.project.use;
  const context = await browser.newContext({ baseURL, viewport, userAgent, deviceScaleFactor, isMobile, hasTouch });
  await context.addInitScript((s) => { localStorage.setItem("party.session", s); }, JSON.stringify({ baseUrl: "", ...session }));
  return context.newPage();
}

interface DmView { id: string; seq: number; state: { combatants: { id: string; visible: boolean; kind: string; owner?: string }[] } }

test.describe("The player's Combat sub-tab with the combat server (0.3.0)", () => {
  test("a reveal reaches the player within 2 s; a hidden creature never reaches the player's phone", async ({ request, browser, baseURL }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "two accounts at 384x832 only");
    test.setTimeout(180_000);
    const dm = await login(request, "admin");
    const player = await login(request, "player");
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Live fight ${Math.random().toString(36).slice(2, 8)}` });
    try {
      const probe = await call(request, dm.token, "GET", `/api/parties/${party.id}/combat`);
      test.skip(probe.status() === 404, "this server has no combat routes (feat/combat-server not in the build)");

      const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
      const hero = (await api<{ summary: { id: string } }>(request, player.token, "POST", "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER })).summary.id;
      await api(request, player.token, "POST", "/api/rpc/load_character", { characterId: hero });
      await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero });

      // The DM's encounter: one creature in sight, one hidden.
      // The bestiary's search when the server has it (feat/bestiary), else the bundled SRD 5.2.1 goblin.
      const search = await call(request, dm.token, "POST", "/api/bestiary/search", { text: "goblin" });
      const found = search.ok() ? ((await search.json()) as { items: { id: string }[] }).items : [];
      const monster = found[0]?.id ?? "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
      const enc = await api<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "Rig ambush (DM's name)", publicName: "Road ambush" });
      await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: monster, hidden: false, publicName: "Scout" });
      await api(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: monster, hidden: true, dmLabel: HIDDEN_LABEL, publicName: HIDDEN_PUBLIC });
      const started = await api<DmView>(request, dm.token, "POST", `/api/encounters/${enc.id}/start`, { requestId: rid() });
      const hidden = started.state.combatants.find((c) => c.kind === "creature" && !c.visible)!;
      const me = started.state.combatants.find((c) => c.kind === "character")!;
      expect(hidden, "the hidden creature is in the DM's view").toBeTruthy();
      const secrets = [hidden.id, HIDDEN_LABEL, HIDDEN_PUBLIC];

      // Everything the player's phone gets from the server is kept, to look for the hidden creature in it.
      const page = await phone(browser, testInfo, baseURL!, player);
      const bodies: string[] = [];
      page.on("response", async (r) => {
        if (!r.url().includes("/api/") || r.url().includes("/api/events")) return;
        try { bodies.push(`${r.url()}\n${JSON.stringify(r.headers())}\n${await r.text()}`); } catch { /* a body already gone */ }
      });
      try {
        await page.goto("/?mock=mobile#/characters");
        await page.getByRole("button", { name: /^Live$/ }).tap({ timeout: 30_000 });
        const pane = page.getByTestId("combat-pane");
        await expect(pane).toBeVisible();
        // Initiative typed from the table.
        const init = pane.getByRole("alertdialog", { name: "Roll initiative" });
        await init.getByRole("button", { name: "Type my table roll" }).tap();
        await init.getByLabel("Initiative total").fill("15");
        await init.getByRole("button", { name: "Send" }).tap();
        await expect(pane.getByLabel("Initiative 15")).toBeVisible();
        await api(request, dm.token, "POST", `/api/combats/${started.id}/roll_initiative`, { requestId: rid(), args: { ids: [], only_missing: true } });
        await api(request, dm.token, "POST", `/api/combats/${started.id}/start`, { requestId: rid(), args: {} });

        const order = pane.getByRole("list", { name: "Turn order" });
        await expect(order.locator(".combat-row")).toHaveCount(2);
        await expect(order).toContainText("Scout");
        const notThere = async () => {
          const html = await page.content();
          for (const s of secrets) expect(html, `the page holds "${s}"`).not.toContain(s);
          for (const b of bodies) for (const s of secrets) expect(b, `an answer holds "${s}"`).not.toContain(s);
        };
        // The DM runs the hidden creature's turn: still nothing on the phone.
        for (let i = 0; i < 3; i++) await api(request, dm.token, "POST", `/api/combats/${started.id}/next_turn`, { requestId: rid(), args: {} });
        await page.waitForTimeout(1_500);
        await notThere();

        // The DM reveals it: the player sees it within about two seconds.
        const t0 = Date.now();
        await api(request, dm.token, "POST", `/api/combats/${started.id}/reveal`, { requestId: rid(), args: { id: hidden.id } });
        await expect(order.locator(".combat-row")).toHaveCount(3, { timeout: LIVE_MS });
        await expect(order).toContainText(HIDDEN_PUBLIC);
        testInfo.annotations.push({ type: "live", description: `reveal shown in ${Date.now() - t0} ms` });
        expect(await page.content()).not.toContain(HIDDEN_LABEL);
        await expect(pane.getByRole("region", { name: "What happened" })).toContainText(`${HIDDEN_PUBLIC} appears.`);

        // A DM change the player can see: on the phone within a second, with no reload.
        const loads = (await page.evaluate(() => performance.getEntriesByType("navigation").length));
        const t1 = Date.now();
        await api(request, dm.token, "POST", `/api/combats/${started.id}/add_condition`, { requestId: rid(), args: { effect: { name: "Prone", target: hidden.id, conditions: ["prone"] } } });
        await expect(order.locator(".combat-row").filter({ hasText: HIDDEN_PUBLIC }).getByText("Prone")).toBeVisible({ timeout: QUICK_MS });
        testInfo.annotations.push({ type: "live", description: `DM condition shown in ${Date.now() - t1} ms` });
        expect(await page.evaluate(() => performance.getEntriesByType("navigation").length), "no reload").toBe(loads);

        // The player summons a creature; it follows the PC, and the DM sees it in the combat.
        await pane.getByRole("button", { name: "Summon…" }).tap();
        const dialog = page.getByRole("dialog", { name: "Summon" });
        await dialog.getByLabel("Name").fill("Spirit Wolf");
        await dialog.getByLabel("Hit points").fill("11");
        await dialog.getByRole("button", { name: "Summon", exact: true }).tap();
        await expect(dialog).toHaveCount(0);
        const names = order.locator(".combat-row .combat-row-name");
        await expect(names.filter({ hasText: "Spirit Wolf (yours)" })).toHaveCount(1);
        const all = await names.allTextContents();
        expect(all.findIndex((n) => n.startsWith("Spirit Wolf")), "right after the PC").toBe(all.findIndex((n) => n.includes("(you)")) + 1);
        const dmView = await api<DmView>(request, dm.token, "GET", `/api/combats/${started.id}`);
        expect(dmView.state.combatants.some((c) => c.kind === "creature" && c.visible)).toBeTruthy();

        // The DM hits the PC: the player applies it to their own character.
        const before = (await api<{ play: { hitPoints: { current: number } } }>(request, player.token, "POST", "/api/rpc/get_snapshot", { characterId: hero })).play.hitPoints.current;
        await api(request, dm.token, "POST", `/api/combats/${started.id}/damage`, { requestId: rid(), args: {
          target: me.id, parts: [{ amount: { kind: "typed", total: 2 }, damage_type: "slashing" }], magical: false, apply_qualified: false, critical_hit: false } });
        const prompt = page.getByRole("alertdialog", { name: "Take 2 slashing damage?" });
        await expect(prompt).toBeVisible({ timeout: LIVE_MS });
        await prompt.getByRole("button", { name: "Apply" }).tap();
        await expect(prompt).toHaveCount(0);
        const after = (await api<{ play: { hitPoints: { current: number } } }>(request, player.token, "POST", "/api/rpc/get_snapshot", { characterId: hero })).play.hitPoints.current;
        expect(after).toBe(Math.max(0, before - 2));
        const o = await measureOverflow(page);
        expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);

        // The DM ends it: the sub-tab goes.
        await api(request, dm.token, "POST", `/api/combats/${started.id}/end`, { requestId: rid(), args: {} });
        await expect(page.getByRole("tab", { name: "Combat" })).toHaveCount(0, { timeout: LIVE_MS });
        // The Live chip is the account's: the rig's seeded player may be in another spec's live combat meanwhile
        // (the tracker's real-server spec runs in parallel), so it must go only when none is live anywhere.
        const parties = (await api<{ parties: { id: string }[] }>(request, player.token, "GET", "/api/parties")).parties;
        const elsewhere = await Promise.all(parties.filter((p) => p.id !== party.id).map(async (p) =>
          ((await api<{ combat: string | null }>(request, player.token, "GET", `/api/parties/${p.id}/combat`)).combat !== null)));
        if (!elsewhere.some(Boolean)) await expect(page.getByRole("button", { name: /^Live$/ })).toHaveCount(0);
      } finally {
        await page.context().close();
      }
    } finally {
      await call(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
