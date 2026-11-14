import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, test, unevenRows } from "./fixtures";

/**
 * 0.3.6 against the REAL server, on the S26 Ultra phone (384x832, android-portrait) and a 1440x900 desktop
 * (desktop-wide):
 * - the Play tab's Companions card under Hit Points: AC, HP with Damage / Heal, conditions, attacks as typed-first
 *   roll buttons; two companions side by side with one height on the desktop, stacked on the phone;
 * - Recently deleted: a deleted party waits on the Account page, Restore brings it back, "Delete forever" asks in an
 *   app dialog and removes it.
 * Screenshots go to PARTY_SHOTS when it is set.
 */

const PROJECTS = new Set(["android-portrait", "desktop-wide"]);

async function call<T>(request: APIRequestContext, auth: string, method: "GET" | "POST" | "DELETE", path: string, data?: object): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data ? { data } : {}) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return (await r.json()) as T;
}
const rpc = <T,>(request: APIRequestContext, auth: string, command: string, data: object) => call<T>(request, auth, "POST", `/api/rpc/${command}`, data);

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const size = testInfo.project.name === "desktop-wide" ? "1440" : "384";
  await page.screenshot({ path: join(dir, `companion-softdelete-${name}-${size}.png`) });
}

const settle = (page: Page) => page.waitForTimeout(500);

async function signIn(page: Page, request: APIRequestContext, characterId?: string) {
  const login = await request.post("/api/auth/login", { data: { username: "player", password: ACCOUNTS.player.password } });
  expect(login.ok()).toBeTruthy();
  const session = (await login.json()) as { token: string; user: unknown };
  await page.addInitScript(([s, id]) => {
    if (!sessionStorage.getItem("phone-rig.seeded")) {
      localStorage.setItem("party.session", s!);
      if (id) sessionStorage.setItem("lk.currentCharacter", id);
      sessionStorage.setItem("phone-rig.seeded", "1");
    }
  }, [JSON.stringify({ baseUrl: "", ...session }), characterId ?? ""]);
  return session.token;
}

test.describe("Companions card and Recently deleted (0.3.6)", () => {
  test("the Companions card on the Play tab", async ({ page, request }, testInfo) => {
    test.skip(!PROJECTS.has(testInfo.project.name), "the phone at 384x832 and the desktop at 1440x900");
    test.setTimeout(180_000);
    const phone = testInfo.project.name === "android-portrait";
    const tap = (l: ReturnType<Page["locator"]>) => (phone ? l.tap() : l.click());
    const login = await request.post("/api/auth/login", { data: { username: "player", password: ACCOUNTS.player.password } });
    const token = ((await login.json()) as { token: string }).token;
    const hero = await rpc<{ id: string }>(request, token, "create_character", {
      request: { name: `Warden ${Math.random().toString(36).slice(2, 6)}`, ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} },
    });
    await rpc(request, token, "add_companion", { characterId: hero.id, creatureId: "ID_SRD51_MONSTER_WOLF", name: "Grip" });
    await rpc(request, token, "add_companion", { characterId: hero.id, creatureId: "ID_SRD51_MONSTER_MASTIFF", name: "Bran" });
    await rpc(request, token, "save_character", { characterId: hero.id });
    await rpc(request, token, "load_character", { characterId: hero.id });
    await signIn(page, request, hero.id);

    await page.goto(phone ? "/?mock=mobile#/play" : "/#/play");
    await expect(page.getByRole("status", { name: /^Hit points: / })).toBeVisible({ timeout: 30_000 });
    const list = page.getByRole("list", { name: "Companions" });
    await list.scrollIntoViewIfNeeded();
    const grip = list.getByRole("listitem", { name: "Grip" });
    const bran = list.getByRole("listitem", { name: "Bran" });
    await expect(grip.getByRole("status", { name: "Grip hit points: 11 of 11" })).toBeVisible();
    await expect(grip).toContainText("AC 13");

    // Desktop: side by side, one height. Phone: one under the other.
    const [a, b] = [await grip.boundingBox(), await bran.boundingBox()];
    if (phone) {
      expect(b!.y).toBeGreaterThanOrEqual(a!.y + a!.height - 1);
    } else {
      expect(Math.abs(a!.y - b!.y)).toBeLessThanOrEqual(1);
      expect(Math.abs(a!.height - b!.height)).toBeLessThanOrEqual(1);
    }
    await settle(page);
    await shot(page, testInfo, "companions");

    // Damage, a condition, kept on this device.
    await grip.getByRole("spinbutton", { name: "Grip amount" }).fill("4");
    await tap(grip.getByRole("button", { name: "Damage Grip" }));
    await expect(grip.getByRole("status", { name: "Grip hit points: 7 of 11" })).toBeVisible();
    await grip.getByRole("combobox", { name: "Add a condition to Grip" }).selectOption("prone");
    await expect(grip.getByRole("list", { name: "Grip conditions" })).toContainText("Prone");
    await page.reload();
    await expect(page.getByRole("list", { name: "Companions" }).getByRole("status", { name: "Grip hit points: 7 of 11" })).toBeVisible({ timeout: 30_000 });

    // Its attack: the table's total first, then the damage.
    await tap(page.getByRole("button", { name: "Grip: Bite, +4 to hit" }));
    const sheet = page.getByRole("dialog", { name: "Grip · Bite" });
    await expect(sheet.getByRole("textbox", { name: "Grip · Bite: my table roll" })).toBeFocused();
    await expect(sheet.getByRole("button", { name: /^Roll for me/ })).toBeVisible();
    await settle(page);
    await shot(page, testInfo, "companion-attack");
    await sheet.getByRole("textbox").fill("15");
    await tap(sheet.getByRole("button", { name: "Send" }));
    await expect(page.getByTestId("roll-result")).toContainText("15");

    // Nothing sideways; the buttons stay inside their card.
    expect((await measureOverflow(page)).page).toBeLessThanOrEqual(0);
    const spill = await page.getByRole("list", { name: "Companions" }).evaluate((el) => Array.from(el.querySelectorAll<HTMLElement>("li.bp-companion")).flatMap((card) => {
      const box = card.getBoundingClientRect();
      return Array.from(card.querySelectorAll<HTMLElement>(".btn")).filter((btn) => {
        const r = btn.getBoundingClientRect();
        return r.left < box.left - 1 || r.right > box.right + 1;
      }).map((btn) => btn.textContent);
    }));
    expect(spill).toEqual([]);
    if (!phone) expect(await unevenRows(page, "main")).toEqual([]);
  });

  test("a deleted party waits in Recently deleted, comes back, and can be deleted forever", async ({ page, request }, testInfo) => {
    test.skip(!PROJECTS.has(testInfo.project.name), "the phone at 384x832 and the desktop at 1440x900");
    test.setTimeout(180_000);
    const phone = testInfo.project.name === "android-portrait";
    const tap = (l: ReturnType<Page["locator"]>) => (phone ? l.tap() : l.click());
    const token = await signIn(page, request);
    await call(request, token, "POST", "/api/me/roles/dm", {});
    const name = `Lantern ${Math.random().toString(36).slice(2, 6)}`;
    const party = await call<{ id: string }>(request, token, "POST", "/api/parties", { name });
    await call(request, token, "POST", `/api/parties/${party.id}/encounters`, { name: "Bridge", publicName: "Bridge" });

    // Delete it in the app: the confirm says where it goes.
    await page.goto(phone ? "/?mock=mobile#/parties" : "/#/parties");
    const partyCard = page.locator(".party-card", { hasText: name });
    await partyCard.scrollIntoViewIfNeeded();
    await tap(partyCard.getByRole("button", { name: "Delete" }));
    const ask = page.getByRole("dialog", { name: `Delete ${name}?` });
    await expect(ask).toContainText("Recently deleted for 30 days");
    await tap(ask.getByRole("button", { name: "Delete party" }));
    await expect(page.getByText(`${name} was moved to Recently deleted.`)).toBeVisible();

    // Account › Recently deleted.
    await page.goto(phone ? "/?mock=mobile#/account" : "/#/account");
    const card = page.locator(".bp-deleted");
    const row = card.locator(".bp-deleted-row", { hasText: name });
    await row.scrollIntoViewIfNeeded();
    await expect(row).toContainText("Party");
    await expect(row).toContainText("Removed in 30 days");
    expect((await measureOverflow(page)).page).toBeLessThanOrEqual(0);
    await settle(page);
    await shot(page, testInfo, "recently-deleted");
    await tap(row.getByRole("button", { name: "Restore" }));
    const restore = page.getByRole("dialog", { name: `Restore ${name}?` });
    await tap(restore.getByRole("button", { name: "Restore" }));
    await expect(page.getByText(`${name} is back.`)).toBeVisible();
    await expect(row).toHaveCount(0);
    const parties = await call<{ parties: { id: string }[] }>(request, token, "GET", "/api/parties");
    expect(parties.parties.map((p) => p.id)).toContain(party.id);
    const encounters = await call<{ encounters: { name: string }[] }>(request, token, "GET", `/api/parties/${party.id}/encounters`);
    expect(encounters.encounters.map((e) => e.name)).toEqual(["Bridge"]);

    // Delete forever, after an app dialog.
    await call(request, token, "DELETE", `/api/parties/${party.id}`);
    await page.reload();
    const again = page.locator(".bp-deleted .bp-deleted-row", { hasText: name });
    await again.scrollIntoViewIfNeeded();
    await tap(again.getByRole("button", { name: "Delete forever" }));
    const forever = page.getByRole("dialog", { name: `Delete ${name} forever?` });
    await expect(forever).toContainText("can't be undone");
    await settle(page);
    await shot(page, testInfo, "delete-forever");
    await tap(forever.getByRole("button", { name: "Delete forever" }));
    await expect(again).toHaveCount(0);
    const left = await call<{ items: { id: string }[] }>(request, token, "GET", "/api/me/deleted");
    expect(left.items.map((i) => i.id)).not.toContain(party.id);
  });
});
