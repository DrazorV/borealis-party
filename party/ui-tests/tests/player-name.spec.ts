import type { APIRequestContext, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * Player Name defaults to the account's username (ui/party/api/playerName.ts, 0.2.2). A character with an empty player
 * name, made through the server, opens on the Manage tab with the username in Player name, and the next save writes it.
 * A typed name is kept. Android portrait only: one account, one character per run.
 */

interface Snapshot { id: string; details: { playerName: string } }

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

/** Saved on file, as an older character or an import with no player name. */
async function emptyPlayer(request: APIRequestContext, baseURL: string, auth: string, name: string): Promise<Snapshot> {
  const s = await rpc<Snapshot>(request, baseURL, auth, "create_character", {
    request: { name, ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} },
  });
  await rpc(request, baseURL, auth, "update_details", { characterId: s.id, patch: { playerName: "" } });
  await rpc(request, baseURL, auth, "save_character", { characterId: s.id });
  await rpc(request, baseURL, auth, "close_character", { characterId: s.id, discard: false });
  return s;
}

async function openManage(page: Page, name: string) {
  await open(page, "/characters");
  await signedInShell(page);
  await page.getByRole("button", { name: new RegExp(`^${name},`) }).tap();
  await page.getByRole("button", { name: `Open ${name}`, exact: true }).tap();
  await expect(page).not.toHaveURL(/#\/characters/, { timeout: 30_000 });
  await page.goto("/?mock=mobile#/manage");
}

test.describe("Player name", () => {
  test("an empty one shows the username on Manage and is saved with it; a typed one is kept", async ({ page, signIn, request, baseURL }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait", "one account, one character");
    const auth = await token(request, baseURL!);
    const name = `Nameless ${testInfo.repeatEachIndex}-${testInfo.retry}`;
    const made = await emptyPlayer(request, baseURL!, auth, name);

    await signIn("player");
    // The bar under the tabs names no account (on a phone it is empty and hidden).
    for (const text of await page.locator(".party-bar").allTextContents()) expect(text).not.toContain("player");
    await openManage(page, name);
    const field = page.getByLabel("Player name", { exact: true });
    await field.scrollIntoViewIfNeeded();
    await expect(field).toHaveValue("player", { timeout: 30_000 });
    expect((await measureOverflow(page)).page, "the page scrolls sideways").toBeLessThanOrEqual(0);
    await page.screenshot({ path: testInfo.outputPath("manage.png") });

    // Typed over, the typed name is what the server keeps, and opening the character again leaves it.
    await field.fill("Table Name");
    await field.blur();
    await expect.poll(async () => (await rpc<Snapshot>(request, baseURL!, auth, "get_snapshot", { characterId: made.id })).details.playerName)
      .toBe("Table Name");
    await rpc(request, baseURL!, auth, "save_character", { characterId: made.id });
    await rpc(request, baseURL!, auth, "close_character", { characterId: made.id, discard: false });
    await openManage(page, name);
    await expect(page.getByLabel("Player name", { exact: true })).toHaveValue("Table Name", { timeout: 30_000 });
  });
});
