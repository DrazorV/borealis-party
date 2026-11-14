import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Locator, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, settled, signedInShell, smallTargets, test } from "./fixtures";

/**
 * The party's edition (0.3.2). `admin` runs the party as its DM, `player` plays a character in it.
 * - New party: the edition is a required choice; the card shows it, and Change edition switches it.
 * - The encounter: Setup shows the party's edition; with most of the characters on the other edition, the Edition
 *   notes offer "Switch party to …", which asks first, and the open encounter follows the change.
 * Runs once on the S26 Ultra's 384x832 (android-portrait) and once at a 1440x900 desktop window.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
type Ruleset = "dnd2014" | "dnd2024";
const LABEL: Record<Ruleset, string> = { dnd2014: "2014 rules", dnd2024: "2024 rules" };
const other = (r: Ruleset): Ruleset => (r === "dnd2014" ? "dnd2024" : "dnd2014");

async function login(request: APIRequestContext, who: AccountName): Promise<string> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return ((await r.json()) as { token: string }).token;
}

async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

const desktop = (testInfo: TestInfo) => !testInfo.project.use.isMobile || testInfo.title.includes("desktop");
const press = (testInfo: TestInfo, target: Locator) => (desktop(testInfo) ? target.click() : target.tap());

async function go(page: Page, testInfo: TestInfo, route: string) {
  if (desktop(testInfo)) {
    await page.goto(`/#${route}`);
    await signedInShell(page);
  } else await open(page, route);
}

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers.filter((s) => !s.includes("enc-chips")), "containers that scroll sideways").toEqual([]);
  if (!desktop(testInfo)) {
    const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
    expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
  }
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (!dir) return;
  const sheet = page.getByRole("dialog");
  if (await sheet.count()) await settled(sheet.first());
  await page.screenshot({ path: join(dir, `${desktop(testInfo) ? "desktop-1440" : testInfo.project.name}-edition-${name}.png`) });
}

async function newPartyOnScreen(page: Page, testInfo: TestInfo, request: APIRequestContext) {
  const dm = await login(request, "admin");
  await api(request, dm, "POST", "/api/me/roles/dm", {});
  const name = `Edition ${desktop(testInfo) ? "desk" : testInfo.project.name} ${Math.random().toString(36).slice(2, 7)}`;
  await go(page, testInfo, "/parties");
  await press(testInfo, page.getByRole("button", { name: /New party/ }));
  const create = page.getByRole("dialog", { name: "New party" });
  await create.getByLabel("Party name").fill(name);
  // A required choice: Create waits for it (pre-selected only from the DM's most recent character).
  const radios = create.getByRole("radiogroup", { name: "Edition" }).getByRole("radio");
  await expect(radios).toHaveText(["2014 rules", "2024 rules"]);
  if ((await create.getByRole("radio", { checked: true }).count()) === 0) {
    await expect(create.getByRole("button", { name: "Create" })).toBeDisabled();
  }
  await press(testInfo, create.getByRole("radio", { name: "2014 rules" }));
  await expect(create.getByRole("radio", { name: "2014 rules" })).toHaveAttribute("aria-checked", "true");
  await fits(page, testInfo, "[role=dialog]");
  await shot(page, testInfo, "new-party");
  await press(testInfo, create.getByRole("button", { name: "Create" }));
  const card = page.getByRole("region", { name });
  await expect(card).toBeVisible();
  await expect(card.locator(".party-edition-line")).toContainText("Edition: 2014 rules");
  const id = (await api<{ parties: { id: string; name: string; ruleset: string }[] }>(request, dm, "GET", "/api/parties")).parties.find((p) => p.name === name)!;
  expect(id.ruleset).toBe("dnd2014");

  await press(testInfo, card.getByRole("button", { name: "Change edition" }));
  const change = page.getByRole("dialog", { name: "Change edition" });
  await expect(change).toContainText("A combat that is running keeps the rules it started with.");
  await fits(page, testInfo, "[role=dialog]");
  await shot(page, testInfo, "change");
  await press(testInfo, change.getByRole("button", { name: "Switch to 2024 rules" }));
  await expect(change).toHaveCount(0);
  await expect(card.locator(".party-edition-line")).toContainText("Edition: 2024 rules");
  await fits(page, testInfo, ".party-parties");
  await shot(page, testInfo, "card");
  await api(request, dm, "DELETE", `/api/parties/${id.id}`);
}

async function switchFromEditionNotes(page: Page, testInfo: TestInfo, request: APIRequestContext) {
  const dm = await login(request, "admin");
  const player = await login(request, "player");
  await api(request, dm, "POST", "/api/me/roles/dm", {});
  const hero = await api<{ summary: { id: string; ruleset: Ruleset } }>(request, player, "POST", "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
  const heroes = hero.summary.ruleset;
  // The party is on the other edition than its only character: the switch is offered.
  const name = `Notes ${desktop(testInfo) ? "desk" : testInfo.project.name} ${Math.random().toString(36).slice(2, 7)}`;
  const party = await api<{ id: string }>(request, dm, "POST", "/api/parties", { name, ruleset: other(heroes) });
  const made = await api<{ code: string }>(request, dm, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
  await api(request, player, "POST", "/api/join", { code: made.code, characterId: hero.summary.id });
  const enc = await api<{ id: string }>(request, dm, "POST", `/api/parties/${party.id}/encounters`, { name: "Edition check" });
  try {
    await go(page, testInfo, `/encounters/${party.id}/${enc.id}`);
    const setup = page.locator(".enc-card").filter({ has: page.getByRole("heading", { name: "Setup" }) });
    await expect(setup.locator(".party-edition-row")).toContainText(`The party's edition: ${LABEL[other(heroes)]}`);
    await expect(setup.getByRole("button", { name: "Change edition" })).toBeVisible();

    await press(testInfo, page.getByRole("button", { name: /^Edition notes/ }).first());
    const notes = page.getByRole("dialog", { name: "Edition notes" });
    await expect(notes).toContainText("1 of the party's characters use the other edition's rules.");
    const fix = notes.getByRole("button", { name: `Switch party to ${LABEL[heroes]}` });
    await expect(fix).toBeVisible();
    await fits(page, testInfo, "[role=dialog]");
    await shot(page, testInfo, "notes");
    await press(testInfo, fix);
    const ask = page.getByRole("dialog", { name: `Switch ${name} to ${LABEL[heroes]}?` });
    await expect(ask).toBeVisible();
    await shot(page, testInfo, "confirm");
    await press(testInfo, ask.getByRole("button", { name: `Switch to ${LABEL[heroes]}` }));
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(setup.locator(".party-edition-row")).toContainText(`The party's edition: ${LABEL[heroes]}`);
    await expect(page.getByRole("button", { name: /^Edition notes/ })).toHaveCount(0);
    const after = await api<{ ruleset: string }>(request, dm, "GET", `/api/parties/${party.id}`);
    expect(after.ruleset).toBe(heroes);

    // Another device of the DM's changes it back: the open encounter follows (the party event).
    await api(request, dm, "PATCH", `/api/parties/${party.id}`, { ruleset: other(heroes) });
    await expect(setup.locator(".party-edition-row")).toContainText(`The party's edition: ${LABEL[other(heroes)]}`);
    await fits(page, testInfo, ".enc-page");
    await shot(page, testInfo, "setup");
  } finally {
    await api(request, dm, "DELETE", `/api/parties/${party.id}`);
  }
}

test.describe("The party's edition on the phone", () => {
  test("New party asks for the edition; the card changes it", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "the S26 Ultra's 384x832");
    await signIn("admin");
    await newPartyOnScreen(page, testInfo, request);
  });

  test("the edition notes switch the party to its characters' edition", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "the S26 Ultra's 384x832");
    await signIn("admin");
    await switchFromEditionNotes(page, testInfo, request);
  });
});

test.describe("The party's edition on a desktop", () => {
  test.use({ viewport: { width: 1440, height: 900 }, isMobile: false, hasTouch: false, deviceScaleFactor: 1 });

  test("desktop: New party asks for the edition; the card changes it", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "one desktop run is enough");
    await signIn("admin");
    await newPartyOnScreen(page, testInfo, request);
  });

  test("desktop: the edition notes switch the party to its characters' edition", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "one desktop run is enough");
    await signIn("admin");
    await switchFromEditionNotes(page, testInfo, request);
  });
});
