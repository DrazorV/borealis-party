import type { APIRequestContext, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * "Skip Personality Choices" (ui/party/api/personality.ts): the selection flow leaves out the background's Personality
 * Trait, Ideal, Bond and Flaw lists. The SRD Acolyte (content/srd-5.1) has two language picks and those four lists.
 * The character is built through the server up to the point where the server's own Next selection is a personality
 * list, then opened on the phone. Android portrait only: the setting is the account's, and the other phones would
 * change it under this test.
 */

const PERSONALITY = /^(Personality Trait|Ideal|Bond|Flaw)$/;

interface Slot { id: string; title: string; tab: string; elementType: string; required: boolean; locked: boolean; chosen: { name: string } | null }
interface Snapshot { id: string; slots: Slot[]; progress: { remaining: number; nextSlotId: string | null; byTab: Record<string, number> } }

async function rpc<T>(request: APIRequestContext, baseURL: string, auth: string, command: string, data: object): Promise<T> {
  const r = await request.post(`${baseURL}/api/rpc/${command}`, { headers: { authorization: `Bearer ${auth}` }, data });
  expect(r.ok(), `${command}: ${await r.text()}`).toBeTruthy();
  return (await r.json()) as T;
}

async function token(request: APIRequestContext, baseURL: string): Promise<string> {
  const r = await request.post(`${baseURL}/api/auth/login`, { data: { username: "player", password: ACCOUNTS.player.password } });
  expect(r.ok()).toBeTruthy();
  return ((await r.json()) as { token: string }).token;
}

/** A 2014 Acolyte built until the server would ask for a personality list next; the server's own snapshot. */
async function acolyte(request: APIRequestContext, baseURL: string, auth: string, name: string): Promise<Snapshot> {
  let s = await rpc<Snapshot>(request, baseURL, auth, "create_character", {
    request: { name, ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} },
  });
  for (let i = 0; i < 60; i++) {
    const next = s.slots.find((x) => x.id === s.progress.nextSlotId);
    if (!next || PERSONALITY.test(next.title)) break;
    const options = await rpc<{ items: { id: string; disabledReason?: string }[] }>(request, baseURL, auth, "get_slot_options", { characterId: s.id, slotId: next.id, limit: 50 });
    const pick = next.elementType === "background" ? options.items.find((o) => o.id === "ID_BACKGROUND_ACOLYTE") : options.items.find((o) => !o.disabledReason);
    expect(pick, `an option for ${next.title}`).toBeTruthy();
    s = await rpc<Snapshot>(request, baseURL, auth, "apply_selection", { characterId: s.id, slotId: next.id, elementId: pick!.id });
  }
  const next = s.slots.find((x) => x.id === s.progress.nextSlotId);
  expect(next?.title, "the server's own Next selection").toMatch(PERSONALITY);
  await rpc(request, baseURL, auth, "save_character", { characterId: s.id });
  await rpc(request, baseURL, auth, "close_character", { characterId: s.id, discard: false });
  return s;
}

const personality = (s: Snapshot) => s.slots.filter((x) => x.tab === "background" && x.elementType === "list" && PERSONALITY.test(x.title));
const openPersonality = (s: Snapshot) => personality(s).filter((x) => x.required && !x.chosen).length;

/** The Build tab's badge count on the phone (0 when it shows none). */
async function buildBadge(page: Page): Promise<number> {
  const label = (await page.getByRole("link", { name: /^Build(,|$)/ }).getAttribute("aria-label")) ?? "";
  return Number(/(\d+) choices remaining/.exec(label)?.[1] ?? 0);
}

/** The slot Next selection goes to, or null when it is not offered. */
async function nextSelection(page: Page): Promise<string | null> {
  const next = page.getByRole("button", { name: "Next selection" });
  if (!(await next.isVisible())) return null;
  await next.tap();
  await expect(page).toHaveURL(/slot=/);
  return new URLSearchParams(page.url().split("?").pop()).get("slot");
}

/** Settings > Character build > Skip Personality Choices, set to `on` (it is saved at once), then Save. */
async function setSkip(page: Page, on: boolean) {
  await page.getByRole("button", { name: "Settings" }).first().tap();
  await page.getByRole("tab", { name: "Character build" }).or(page.getByRole("button", { name: "Character build" })).first().tap();
  const box = page.getByRole("checkbox", { name: "Skip Personality Choices" });
  await box.scrollIntoViewIfNeeded();
  await expect(box).toBeVisible();
  const o = await measureOverflow(page);
  expect(o.page, "the settings scroll sideways").toBeLessThanOrEqual(0);
  if ((await box.isChecked()) !== on) await box.tap();
  await expect(box).toBeChecked({ checked: on });
  await expect(box).toBeEnabled();
  await page.getByRole("button", { name: "Save" }).tap();
  await expect(box).toBeHidden();
}

async function openCharacter(page: Page, name: string) {
  await open(page, "/characters");
  await signedInShell(page);
  await page.getByRole("button", { name: new RegExp(`^${name},`) }).tap();
  await page.getByRole("button", { name: `Open ${name}`, exact: true }).tap();
  await expect(page).not.toHaveURL(/#\/characters/, { timeout: 30_000 });
  await page.goto("/?mock=mobile#/builder/race");
  await expect(page.locator(".pick[data-slot-id]").first()).toBeVisible({ timeout: 30_000 });
}

test.describe("Personality choices and the selection flow", () => {
  test("Next selection and the badges skip them; they can still be picked; the setting brings them back", async ({ page, signIn, request, baseURL }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "the setting is the account's: one phone changes it");
    const auth = await token(request, baseURL!);
    await rpc(request, baseURL!, auth, "save_settings", { patch: { skipPersonalityChoices: true } });
    const name = `Acolyte ${testInfo.repeatEachIndex}-${testInfo.retry}`;
    const host = await acolyte(request, baseURL!, auth, name);
    const hostBuild = host.progress.remaining - (host.progress.byTab.spells ?? 0);
    expect(openPersonality(host)).toBe(5); // two traits, an ideal, a bond and a flaw

    await signIn("player");
    await openCharacter(page, name);
    // The badges leave them out.
    await expect.poll(() => buildBadge(page)).toBe(hostBuild - 5);
    // Next selection goes elsewhere (or is not offered, with nothing else open on Build).
    const ids = personality(host).map((x) => x.id);
    const first = await nextSelection(page);
    expect(ids).not.toContain(first);
    await page.screenshot({ path: testInfo.outputPath("skipped.png") });

    // The Background section still lists them, and a trait can be picked there.
    await page.goto("/?mock=mobile#/builder/background");
    const trait = page.locator(`.pick[data-slot-id="${ids[0]}"]`);
    await trait.scrollIntoViewIfNeeded();
    await expect(trait).toBeVisible();
    expect((await measureOverflow(page)).page, "the page scrolls sideways").toBeLessThanOrEqual(0);
    await page.screenshot({ path: testInfo.outputPath("background.png") });
    await trait.getByText("Nothing can shake my optimistic attitude.").tap();
    await page.locator(".bp-choose-btn").tap();
    await expect.poll(async () => {
      const now = await rpc<Snapshot>(request, baseURL!, auth, "get_snapshot", { characterId: host.id });
      return personality(now).filter((x) => x.chosen).length;
    }, { message: "the trait is picked on the server" }).toBe(1);
    await expect.poll(() => buildBadge(page)).toBe(hostBuild - 5);

    // Turned off, the flow asks for them again: Next selection goes to one, and the badge counts the four still open.
    await setSkip(page, false);
    await expect.poll(() => buildBadge(page)).toBe(hostBuild - 1);
    await page.screenshot({ path: testInfo.outputPath("asked.png") });
    expect(ids).toContain(await nextSelection(page));
    // The picked trait stayed.
    const picked = await rpc<Snapshot>(request, baseURL!, auth, "get_snapshot", { characterId: host.id });
    expect(personality(picked).map((x) => x.chosen?.name).filter(Boolean)).toEqual(["Nothing can shake my optimistic attitude."]);

    // And on again.
    await setSkip(page, true);
    await expect.poll(() => buildBadge(page)).toBe(hostBuild - 5);
    expect(ids).not.toContain(await nextSelection(page));
  });
});
