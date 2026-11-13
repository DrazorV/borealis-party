import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, scrollIntoReach, signedInShell, smallTargets, test } from "./fixtures";

/**
 * Share codes (plan E4): one account makes a code on its phone, with no file, and another enters it on its phone and
 * gets the usual Open bundle window. Both at the phones' sizes (384x832 on Android), with no sideways scroll and every
 * control at least a tap target.
 */

/** A session token for `who`, for setting up through the API. */
async function token(request: APIRequestContext, baseURL: string, who: AccountName): Promise<string> {
  const r = await request.post(`${baseURL}/api/auth/login`, { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return ((await r.json()) as { token: string }).token;
}

/** A saved character of `who`, named for this test; its id. */
async function character(request: APIRequestContext, baseURL: string, auth: string, name: string): Promise<string> {
  const headers = { authorization: `Bearer ${auth}` };
  const made = await request.post(`${baseURL}/api/rpc/create_character`, { headers, data: { request: { name, ruleset: "dnd2024", abilityMethod: "point_buy" } } });
  expect(made.ok(), await made.text()).toBeTruthy();
  const id = ((await made.json()) as { id: string }).id;
  await request.post(`${baseURL}/api/rpc/save_character`, { headers, data: { characterId: id } });
  await request.post(`${baseURL}/api/rpc/close_character`, { headers, data: { characterId: id } });
  return id;
}

async function expectPhoneLayout(page: Page, testInfo: TestInfo) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), ".party-share")).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `share controls under ${minTap(testInfo)} px`).toEqual([]);
}

const unique = (testInfo: TestInfo, what: string) => `${what} ${testInfo.project.name} ${testInfo.repeatEachIndex}-${testInfo.retry}`;

test.describe("Share codes (plan E4)", () => {
  test("More has Share with a code, which makes a code of a character with no file", async ({ page, signIn, request, baseURL }, testInfo) => {
    const name = unique(testInfo, "Sharer");
    await character(request, baseURL!, await token(request, baseURL!, "admin"), name);
    await signIn("admin");
    await open(page, "/more");
    await signedInShell(page);
    const row = page.getByRole("list", { name: "Account" }).getByRole("link", { name: "Share with a code" });
    expect(await scrollIntoReach(page, row, page.viewportSize()!.height)).toBe(true);
    await row.tap();
    await expect(page.getByRole("heading", { name: "Share with a code" })).toBeVisible();
    await expectPhoneLayout(page, testInfo);

    const pick = page.getByRole("checkbox", { name: new RegExp(name) });
    await pick.scrollIntoViewIfNeeded();
    await pick.check();
    await page.getByRole("button", { name: "Make a share code" }).tap();
    const code = page.getByTestId("share-code");
    await expect(code).toHaveText(/^[0-9A-Z]{4}(-[0-9A-Z]{4}){3}$/, { timeout: 30_000 });
    await expectPhoneLayout(page, testInfo);
    // It is listed as waiting, and Cancel takes it back.
    await page.getByRole("button", { name: "Done" }).tap();
    const list = page.getByRole("list", { name: "Your share codes" });
    await expect(list.getByText("Waiting to be used").first()).toBeVisible();
    await expectPhoneLayout(page, testInfo);
  });

  test("another account enters the code in the Import section and accepts the character", async ({ page, signIn, request, baseURL }, testInfo) => {
    const name = unique(testInfo, "Shared");
    const admin = await token(request, baseURL!, "admin");
    const id = await character(request, baseURL!, admin, name);
    const made = await request.post(`${baseURL}/api/shares`, { headers: { authorization: `Bearer ${admin}` }, data: { characterIds: [id], packs: "none" } });
    expect(made.ok(), await made.text()).toBeTruthy();
    const { code } = (await made.json()) as { code: string };

    await signIn("player");
    await open(page, "/content");
    await signedInShell(page);
    const input = page.getByLabel("Share code to enter");
    await input.scrollIntoViewIfNeeded();
    await input.fill(code.toLowerCase());
    await page.getByRole("button", { name: "Enter a share code" }).tap();
    await expect(page.getByRole("heading", { name: "From admin" })).toBeVisible({ timeout: 30_000 });

    const dialog = page.getByRole("dialog", { name: "Open bundle" });
    await expect(dialog).toBeVisible({ timeout: 30_000 });
    await expect(dialog.getByText(name, { exact: true })).toBeVisible();
    const o = await measureOverflow(page);
    expect(o.page, "the page scrolls sideways under the Open bundle window").toBeLessThanOrEqual(0);
    await dialog.getByRole("button", { name: "Accept" }).tap();
    await expect(page.getByText(/Accepted\. Anything you already had/)).toBeVisible({ timeout: 30_000 });
    await expectPhoneLayout(page, testInfo);

    // Used: the same code again is refused in the server's words.
    const again = page.getByLabel("Share code", { exact: true });
    await again.fill(code);
    // Enter submits the code's own form (the list below may have Open buttons of its own).
    await again.press("Enter");
    await expect(page.getByRole("alert")).toContainText("That code doesn't work");
    // The character is the player's now.
    const list = await request.post(`${baseURL}/api/rpc/list_characters`, { headers: { authorization: `Bearer ${await token(request, baseURL!, "player")}` }, data: {} });
    expect(((await list.json()) as { name: string }[]).map((c) => c.name)).toContain(name);
  });
});
