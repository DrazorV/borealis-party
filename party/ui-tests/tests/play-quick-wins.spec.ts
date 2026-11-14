import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, test, unevenRows } from "./fixtures";

/**
 * The player's quick wins on the Play tab (0.3.5) against the REAL server, on the S26 Ultra phone (384x832,
 * android-portrait) and a 1440x900 desktop (desktop-wide):
 * - every d20 roll asks for the table's total first ("Roll for me" second) and lands in the roll history;
 * - damage while concentrating asks for a concentration save, DC 10 or half the damage; a fail ends it;
 * - at 0 hit points, outside combat, the death saves card counts typed saves to Stable;
 * - quick counters fill the tracker form in one tap and are saved on the character.
 * Screenshots go to PARTY_SHOTS when it is set.
 */

const PROJECTS = new Set(["android-portrait", "desktop-wide"]);

async function api<T>(request: APIRequestContext, auth: string, command: string, data: object): Promise<T> {
  const r = await request.post(`/api/rpc/${command}`, { headers: { authorization: `Bearer ${auth}` }, data });
  expect(r.ok(), `${command}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return (await r.json()) as T;
}

interface Slot { id: string; elementType: string }
interface Snapshot {
  id: string; name: string; slots: Slot[]; progress: { nextSlotId: string | null };
  spellcasting: { classes: { classSlotId: string; spells: { id: string; name: string; prepared: boolean }[] }[] };
  play: { hitPoints: { current: number; max: number }; concentration: { id: string } | null; concentrationSpells: { id: string; name: string }[]; trackers: { name: string; max: number; reset: string }[] };
}

/** A level 1 SRD cleric with Bless prepared (a concentration spell), built through the server as the builder would. */
async function cleric(request: APIRequestContext, auth: string, name: string): Promise<Snapshot> {
  const rpc = <T,>(command: string, data: object) => api<T>(request, auth, command, data);
  let s = await rpc<Snapshot>("create_character", { request: { name, ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} } });
  for (let i = 0; i < 40; i++) {
    const next = s.slots.find((x) => x.id === s.progress.nextSlotId);
    if (!next) break;
    const options = await rpc<{ items: { id: string; disabledReason?: string }[] }>("get_slot_options", { characterId: s.id, slotId: next.id, limit: 80 });
    const pick = next.elementType === "class" ? options.items.find((o) => /CLERIC/i.test(o.id)) : options.items.find((o) => !o.disabledReason);
    if (!pick) break;
    s = await rpc<Snapshot>("apply_selection", { characterId: s.id, slotId: next.id, elementId: pick.id });
  }
  const block = s.spellcasting.classes[0]!;
  const bless = block.spells.find((x) => x.name === "Bless")!;
  if (!bless.prepared) s = await rpc<Snapshot>("set_spell_prepared", { characterId: s.id, classSlotId: block.classSlotId, spellId: bless.id, prepared: true });
  await rpc("save_character", { characterId: s.id });
  return s;
}

/** Dialogs slide up on phones: let them settle before a screenshot. */
async function settle(page: Page) {
  await page.waitForTimeout(500);
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const size = testInfo.project.name === "desktop-wide" ? "1440" : "384";
  await page.screenshot({ path: join(dir, `play-quick-wins-${name}-${size}.png`) });
}

test.describe("Play tab quick wins (0.3.5)", () => {
  test("typed rolls first, the concentration save, death saves and quick counters", async ({ page, request }, testInfo) => {
    test.skip(!PROJECTS.has(testInfo.project.name), "the phone at 384x832 and the desktop at 1440x900");
    test.setTimeout(180_000);
    const phone = testInfo.project.name === "android-portrait";
    const login = await request.post("/api/auth/login", { data: { username: "player", password: ACCOUNTS.player.password } });
    expect(login.ok()).toBeTruthy();
    const session = (await login.json()) as { token: string; user: unknown };
    const hero = await cleric(request, session.token, `Quill ${Math.random().toString(36).slice(2, 6)}`);
    await api(request, session.token, "load_character", { characterId: hero.id });
    await page.addInitScript(([s, id]) => {
      if (!sessionStorage.getItem("phone-rig.seeded")) {
        localStorage.setItem("party.session", s!);
        sessionStorage.setItem("lk.currentCharacter", id!);
        sessionStorage.setItem("phone-rig.seeded", "1");
      }
    }, [JSON.stringify({ baseUrl: "", ...session }), hero.id]);
    const tap = (l: ReturnType<Page["locator"]>) => (phone ? l.tap() : l.click());

    await page.goto(phone ? "/?mock=mobile#/play" : "/#/play");
    await expect(page.getByRole("status", { name: /^Hit points: / })).toBeVisible({ timeout: 30_000 });

    // 1. Type my d20: a skill on the Abilities sub-tab asks for the table's total first.
    await tap(page.getByRole("tab", { name: "Abilities" }));
    await tap(page.getByRole("button", { name: /^Roll Perception check/ }).first());
    const sheet = page.getByRole("dialog", { name: "Perception check" });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole("textbox", { name: "Perception check: my table roll" })).toBeFocused();
    await expect(sheet.getByRole("button", { name: /^Roll for me/ })).toBeVisible();
    await settle(page);
    await shot(page, testInfo, "typed-roll");
    await sheet.getByRole("textbox").fill("17");
    await tap(sheet.getByRole("button", { name: "Send" }));
    await expect(page.getByTestId("roll-result")).toContainText("17");
    await expect(page.getByTestId("roll-result")).toContainText("rolled at the table");
    await tap(page.getByRole("button", { name: /^Recent rolls \(1\)/ }));
    const history = page.getByRole("list", { name: "Recent rolls" });
    await expect(history).toContainText("Perception check");
    await settle(page);
    await shot(page, testInfo, "roll-history");
    await page.keyboard.press("Escape");

    // 2. Concentration: Bless, then 6 damage asks for a DC 10 save; Failed ends it.
    const bless = hero.play.concentrationSpells.find((x) => x.name === "Bless") ?? hero.play.concentrationSpells[0]!;
    await api(request, session.token, "set_concentration", { characterId: hero.id, spellId: bless.id });
    await page.reload();
    await expect(page.getByRole("region", { name: "Concentration" })).toBeVisible({ timeout: 30_000 });
    await page.getByRole("spinbutton", { name: "Hit point amount" }).fill("6");
    await tap(page.getByRole("button", { name: "Take damage" }));
    const save = page.getByRole("alertdialog", { name: "Concentration save, DC 10" });
    await expect(save).toBeVisible();
    await expect(save).toContainText("(6 damage)");
    expect((await measureOverflow(page)).page).toBeLessThanOrEqual(0);
    await shot(page, testInfo, "concentration-save");
    await expect(save.getByRole("textbox")).toHaveCount(0);
    await tap(save.getByRole("button", { name: "Failed" }));
    await expect(page.getByRole("region", { name: "Concentration" })).toHaveCount(0);
    await expect.poll(async () => (await api<Snapshot>(request, session.token, "get_snapshot", { characterId: hero.id })).play.concentration).toBeNull();

    // 3. Death saves outside combat: down to 0, a success and a natural 1 (two failures), then two more successes.
    const left = (await api<Snapshot>(request, session.token, "get_snapshot", { characterId: hero.id })).play.hitPoints.current;
    await page.getByRole("spinbutton", { name: "Hit point amount" }).fill(String(left));
    await tap(page.getByRole("button", { name: "Take damage" }));
    const card = page.getByRole("region", { name: "Death saves" });
    await expect(card).toBeVisible();
    await expect(card).toContainText("Dying");
    for (const outcome of ["Success", "Natural 1"]) {
      await tap(card.getByRole("button", { name: outcome }));
    }
    await expect(card.getByRole("img", { name: "Failures: 2 of 3" })).toBeVisible();
    expect((await measureOverflow(page)).page).toBeLessThanOrEqual(0);
    // The outcome buttons stay inside the card and show their whole label (nothing squeezed onto one phone row).
    const spill = await card.evaluate((el) => {
      const box = el.getBoundingClientRect();
      return Array.from(el.querySelectorAll<HTMLElement>(".roll-outcomes-row > .btn"))
        .filter((b) => { const r = b.getBoundingClientRect(); return r.left < box.left - 1 || r.right > box.right + 1 || b.scrollWidth > b.clientWidth + 1; })
        .map((b) => b.textContent);
    });
    expect(spill).toEqual([]);
    await shot(page, testInfo, "death-saves");
    for (const outcome of ["Success", "Success"]) {
      await tap(card.getByRole("button", { name: outcome }));
    }
    await expect(card).toContainText("Stable");
    await shot(page, testInfo, "death-stable");
    await tap(page.getByRole("button", { name: "Regain 1 hit point" }));
    await expect(card).toHaveCount(0);

    // 4. Quick counters: Luck in one tap, saved on the character.
    const quick = page.getByRole("group", { name: "Quick counters" }).first();
    await quick.scrollIntoViewIfNeeded();
    await tap(quick.getByRole("button", { name: "Luck" }));
    const dialog = page.getByRole("dialog", { name: "Add tracker" });
    await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue("Luck");
    await settle(page);
    await shot(page, testInfo, "quick-counter");
    await tap(dialog.getByRole("button", { name: "Save" }));
    await expect(page.getByRole("list", { name: "Trackers" })).toContainText("Luck");
    await expect.poll(async () => (await api<Snapshot>(request, session.token, "get_snapshot", { characterId: hero.id })).play.trackers.map((t) => t.name)).toContain("Luck");
    await page.getByRole("list", { name: "Trackers" }).scrollIntoViewIfNeeded();
    await shot(page, testInfo, "trackers");

    // Nothing scrolls sideways; boxes side by side on the desktop keep one height.
    expect((await measureOverflow(page)).page).toBeLessThanOrEqual(0);
    if (!phone) expect(await unevenRows(page, "main")).toEqual([]);
  });
});
