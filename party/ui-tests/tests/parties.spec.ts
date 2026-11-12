import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, scrollIntoReach, signedInShell, smallTargets, test } from "./fixtures";

/**
 * Parties (plan 4.1, release 0.2.0), with two accounts on the throwaway server: `admin` turns on Dungeon Master tools
 * and runs the party (any account can; admin is just the second one the rig seeds), `player` joins with a code or an
 * invite. Every test makes its own party and imports its own character, so the phones can run at once against one
 * server. The phones (384x832, 832x384, 390x844) run all of it; the last group runs once at a desktop size.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "import/fine.dnd5e")).toString("base64");

async function token(request: APIRequestContext, who: AccountName): Promise<string> {
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

/** A fresh character for `who`; answers its id. */
async function newCharacter(request: APIRequestContext, auth: string): Promise<string> {
  const report = await api<{ summary: { id: string } }>(request, auth, "POST", "/api/rpc/import_character", { fileName: "fine.dnd5e", contentBase64: CHARACTER });
  return report.summary.id;
}

/** A party with a unique name, run by `admin` (DM tools switched on). */
async function newParty(request: APIRequestContext, testInfo: TestInfo): Promise<{ id: string; name: string; dm: string }> {
  const dm = await token(request, "admin");
  await api(request, dm, "POST", "/api/me/roles/dm", {});
  const name = `Party ${testInfo.project.name.slice(0, 8)} ${testInfo.testId.slice(0, 6)}${testInfo.retry}`;
  const p = await api<{ id: string }>(request, dm, "POST", "/api/parties", { name });
  return { id: p.id, name, dm };
}

async function noSidewaysScroll(page: Page) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  expect(o.clipped, "cut off at the right edge").toEqual([]);
}

async function bigEnough(page: Page, testInfo: TestInfo, within: string) {
  const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
}

const partyCard = (page: Page, name: string) => page.getByRole("region", { name });

test.describe("Parties on a phone (plan 4.1)", () => {
  test("More has a Parties row; the page fits the screen", async ({ page, signIn }, testInfo) => {
    await signIn("player");
    await open(page, "/more");
    await signedInShell(page);
    const row = page.getByRole("list", { name: "Account" }).getByRole("link", { name: "Parties", exact: true });
    expect(await scrollIntoReach(page, row, page.viewportSize()!.height)).toBe(true);
    await row.tap();
    await expect(page.getByRole("heading", { name: "Join a party" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "My parties" })).toBeVisible();
    await noSidewaysScroll(page);
    await bigEnough(page, testInfo, ".party-parties");
  });

  test("the Account page's Dungeon Master tools switch", async ({ page, signIn }, testInfo) => {
    await signIn("player");
    await open(page, "/account");
    const toggle = page.getByRole("switch", { name: "Dungeon Master tools" });
    await expect(toggle).toBeEnabled();
    // Left as found: other tests on other phones read the same account.
    const was = await toggle.getAttribute("aria-checked");
    await toggle.tap();
    await expect(toggle).toHaveAttribute("aria-checked", was === "true" ? "false" : "true");
    await bigEnough(page, testInfo, ".card:has([role=switch])");
    await toggle.tap();
    await expect(toggle).toHaveAttribute("aria-checked", String(was));
  });

  test("DM: create a party, make a join code, then see the player who joined", async ({ page, signIn, request }, testInfo) => {
    await signIn("admin");
    const dm = await token(request, "admin");
    await api(request, dm, "POST", "/api/me/roles/dm", {});
    await open(page, "/parties");
    const name = `Party ${testInfo.project.name.slice(0, 8)} ${testInfo.testId.slice(0, 6)}${testInfo.retry}`;
    await page.getByRole("button", { name: /New party/ }).tap();
    const create = page.getByRole("dialog", { name: "New party" });
    await create.getByLabel("Party name").fill(name);
    await create.getByRole("button", { name: "Create" }).tap();
    const card = partyCard(page, name);
    await expect(card).toBeVisible();

    await card.getByRole("button", { name: /Invite/ }).tap();
    const invite = page.getByRole("dialog", { name: `Invite to ${name}` });
    await invite.getByRole("tab", { name: "Join code" }).tap();
    await invite.getByRole("button", { name: "Make a join code" }).tap();
    const code = (await invite.getByLabel("Join code").textContent())!.trim();
    expect(code).toMatch(/^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$/);
    await noSidewaysScroll(page);
    await bigEnough(page, testInfo, "[role=dialog]");
    await invite.getByRole("button", { name: "Done" }).tap();
    await expect(card.getByText(/1 use left/)).toBeVisible();

    // The player joins with the code (through the API here; the next test does it on screen).
    const player = await token(request, "player");
    const character = await newCharacter(request, player);
    await api(request, player, "POST", "/api/join", { code, characterId: character });

    await page.reload();
    const member = partyCard(page, name).getByRole("listitem").filter({ hasText: "player" });
    await expect(member).toBeVisible();
    await expect(member.getByRole("button", { name: "Remove" })).toBeVisible();
    await noSidewaysScroll(page);
    await bigEnough(page, testInfo, ".party-parties");

    await member.getByRole("button", { name: "Remove" }).tap();
    await page.getByRole("dialog").getByRole("button", { name: "Remove" }).tap();
    await expect(partyCard(page, name).getByText("No characters yet. Invite your players.")).toBeVisible();

    await partyCard(page, name).getByRole("button", { name: /Delete/ }).tap();
    await page.getByRole("dialog").getByRole("button", { name: "Delete party" }).tap();
    await expect(partyCard(page, name)).toHaveCount(0);
  });

  test("player: enter a code, pick a character, see the party, leave it", async ({ page, signIn, request }, testInfo) => {
    const party = await newParty(request, testInfo);
    const made = await api<{ code: string }>(request, party.dm, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
    const player = await token(request, "player");
    const character = await newCharacter(request, player);

    await signIn("player");
    await open(page, "/parties");
    await page.getByLabel("Join code").fill(made.code.toLowerCase());
    await page.getByRole("button", { name: "Join", exact: true }).tap();
    const pick = page.getByRole("dialog", { name: `Join ${party.name}` });
    await expect(pick).toBeVisible();
    await pick.locator(`input[value="${character}"]`).check();
    await noSidewaysScroll(page);
    await bigEnough(page, testInfo, "[role=dialog]");
    const joinButton = pick.getByRole("button", { name: "Join with this character" });
    expect(await scrollIntoReach(page, joinButton, page.viewportSize()!.height)).toBe(true);
    await joinButton.tap();

    const card = partyCard(page, party.name);
    await expect(card).toBeVisible();
    await expect(card.getByText("Player", { exact: true })).toBeVisible();
    await expect(card.getByText(/DM: admin/)).toBeVisible();
    const mine = card.getByRole("listitem").filter({ hasText: "your character" });
    await expect(mine).toHaveCount(1);
    await noSidewaysScroll(page);
    await bigEnough(page, testInfo, ".party-parties");

    // The same character can't join a second party (D19): it shows, but can't be picked.
    const other = await newParty(request, testInfo);
    await api(request, other.dm, "POST", `/api/parties/${other.id}/invites`, { kind: "username", username: "player" });
    await page.reload();
    await page.getByRole("listitem", { name: `Invite to ${other.name}` }).getByRole("button", { name: "Accept" }).tap();
    const second = page.getByRole("dialog", { name: `Join ${other.name}` });
    await expect(second.locator(`input[value="${character}"]`)).toBeDisabled();
    await second.getByRole("button", { name: "Cancel" }).tap();

    await mine.getByRole("button", { name: "Leave" }).tap();
    await page.getByRole("dialog").getByRole("button", { name: "Leave" }).tap();
    await expect(partyCard(page, party.name)).toHaveCount(0);

    // Clean up the parties this test made.
    await api(request, party.dm, "DELETE", `/api/parties/${party.id}`);
    await api(request, party.dm, "DELETE", `/api/parties/${other.id}`);
  });
});

test.describe("Parties at a desktop size", () => {
  test.use({ viewport: { width: 1280, height: 800 }, isMobile: false, hasTouch: false, deviceScaleFactor: 1 });

  test("the account bar has Parties; DM and player views side by side in one page", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "one desktop run is enough");
    const party = await newParty(request, testInfo);
    const dmCharacter = await newCharacter(request, party.dm);
    await api(request, party.dm, "POST", `/api/parties/${party.id}/members`, { characterId: dmCharacter });

    await signIn("admin");
    await page.goto("/#/characters");
    await signedInShell(page);
    await page.locator(".party-bar").getByRole("button", { name: "Parties" }).click();
    const card = partyCard(page, party.name);
    await expect(card).toBeVisible();
    await expect(card.getByText("DM", { exact: true })).toBeVisible();
    // The DM's own character (D4) is theirs to withdraw, not to remove.
    const own = card.getByRole("listitem").filter({ hasText: "your character" });
    await expect(own.getByRole("button", { name: "Leave" })).toBeVisible();
    await expect(own.getByRole("button", { name: "Remove" })).toHaveCount(0);
    await noSidewaysScroll(page);

    await page.locator(".party-bar").getByRole("button", { name: "Account" }).click();
    await expect(page.getByRole("switch", { name: "Dungeon Master tools" })).toHaveAttribute("aria-checked", "true");
    await api(request, party.dm, "DELETE", `/api/parties/${party.id}`);
  });
});
