import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Locator, Page, TestInfo } from "@playwright/test";
import { SEEDED } from "./accounts";
import { expect, signedInShell, test } from "./fixtures";

/**
 * Additional Content follows its changes without a reload (0.2.3, ui/party/content/contentSync.ts): a removed pack or
 * index file leaves its list at once, and a change made somewhere else (another tab or device, here the server API)
 * shows within about a second, through the server's `party://content-changed`. Each project has an account of its own.
 * Run on the phones and a desktop window.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const PACK = readFileSync(join(FIXTURES, "d4-pack.xml")).toString("base64");
/** Never fetched: adding an index only stores it (the address must resolve). */
const INDEX = "https://raw.githubusercontent.com/example/refresh-test/main/refresh.index";
/** How soon a list must follow a change. */
const SOON_MS = 1000;

// One account per project: its tests take turns.
test.describe.configure({ mode: "serial" });

const desktop = (testInfo: TestInfo) => testInfo.project.name === "desktop";

interface Session { token: string; user: unknown }

async function rpc(request: APIRequestContext, baseURL: string, token: string, command: string, args: unknown) {
  const r = await request.post(`${baseURL}/api/rpc/${command}`, { data: args, headers: { authorization: `Bearer ${token}` } });
  expect(r.ok(), `${command}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return r.json() as Promise<unknown>;
}

/**
 * This project's own account (`cr-<project>`, seeded by server/start.ts: sign-ups are rate limited per address), with
 * no custom packs or index files left from an earlier test. The tests of a project share it, one after the other.
 */
async function ownAccount(request: APIRequestContext, baseURL: string, project: string): Promise<Session> {
  const account = SEEDED.find((a) => a.name === `cr-${project}`)!;
  const r = await request.post(`${baseURL}/api/auth/login`, { data: { username: account.name, password: account.password } });
  expect(r.ok(), `login: ${r.status()} ${await r.text()}`).toBeTruthy();
  const session = (await r.json()) as Session;
  const packs = (await rpc(request, baseURL, session.token, "list_packs", {})) as { id: string; kind: string }[];
  const left = packs.filter((p) => p.kind !== "bundled").map((p) => p.id);
  if (left.length > 0) {
    const out = await request.post(`${baseURL}/api/content/remove-packs`, { data: { packIds: left }, headers: { authorization: `Bearer ${session.token}` } });
    expect(out.ok(), await out.text()).toBeTruthy();
  }
  for (const i of (await rpc(request, baseURL, session.token, "list_indexes", {})) as { url: string }[]) {
    await rpc(request, baseURL, session.token, "remove_index", { url: i.url });
  }
  return session;
}

async function signInAs(page: Page, session: Session) {
  await page.addInitScript((s) => {
    if (!sessionStorage.getItem("content-refresh.seeded")) {
      localStorage.setItem("party.session", s);
      sessionStorage.setItem("content-refresh.seeded", "1");
    }
  }, JSON.stringify({ baseUrl: "", ...session }));
}

/** Additional Content, signed in; a marker on the window shows later that the page never reloaded. */
async function openContent(page: Page, testInfo: TestInfo): Promise<void> {
  await page.goto(desktop(testInfo) ? "/#/content" : "/?mock=mobile#/content");
  await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
  await signedInShell(page);
  await page.evaluate(() => { (window as unknown as { contentRefreshMark: number }).contentRefreshMark = 1; });
}

const notReloaded = (page: Page) => page.evaluate(() => (window as unknown as { contentRefreshMark?: number }).contentRefreshMark === 1);

/**
 * The removable packs shown (built-in ones have no Remove). In the phone layout packs come by origin, folded (Custom is
 * the group of imported XML), else in one list. A desktop window can start with one and switch to the other once the
 * account's settings load, so each count looks again and opens the Custom group when there is one.
 */
const packRows = (page: Page): Locator =>
  page.locator('ul[aria-label="Custom packs"] > li, ul[aria-label="Content packs"] > li').filter({ has: page.getByRole("button", { name: "Remove" }) });

async function packCount(page: Page): Promise<number> {
  const group = page.getByRole("button", { name: /^Custom \(/ });
  if ((await group.count()) > 0 && (await group.getAttribute("aria-expanded")) === "false") await group.click();
  return packRows(page).count();
}

/** Waits until `n` removable packs show. */
const showsPacks = (page: Page, n: number) => expect.poll(() => packCount(page), { timeout: 10_000 }).toBe(n);

/** Milliseconds until `check` passes (polled every 25 ms), failing after 10 s. */
async function timeUntil(check: () => Promise<boolean>): Promise<number> {
  const start = Date.now();
  while (!(await check())) {
    if (Date.now() - start > 10_000) throw new Error("the list never followed the change");
    await new Promise((r) => setTimeout(r, 25));
  }
  return Date.now() - start;
}

test("a removed pack leaves the list at once, and the server's answer keeps it gone", async ({ page, request, baseURL }, testInfo) => {
  const session = await ownAccount(request, baseURL!, testInfo.project.name);
  for (const fileName of ["refresh-a.xml", "refresh-b.xml"]) {
    await rpc(request, baseURL!, session.token, "import_pack", { request: { fileName, contentBase64: PACK } });
  }
  await signInAs(page, session);
  await openContent(page, testInfo);
  await showsPacks(page, 2);

  // Both packs come from the same file, so they have the same name: the first row.
  const row = packRows(page).first();
  await row.getByRole("button", { name: "Remove" }).click();
  const dialog = page.getByRole("dialog", { name: "Remove pack" });
  await dialog.getByRole("button", { name: "Remove" }).click();
  const ms = await timeUntil(async () => (await packCount(page)) === 1);
  testInfo.annotations.push({ type: "timing", description: `pack removed from the list after ${ms} ms` });
  expect(ms, "the pack left the list").toBeLessThan(SOON_MS);
  // Done on the server too, and it stays gone after the server's own answer and the refetch.
  await expect.poll(async () => ((await rpc(request, baseURL!, session.token, "list_packs", {})) as { kind: string }[]).filter((p) => p.kind !== "bundled").length).toBe(1);
  await page.waitForTimeout(500);
  await showsPacks(page, 1);
  expect(await notReloaded(page), "the page reloaded").toBe(true);
});

test("packs and index files added or removed elsewhere show within a second, without a reload", async ({ page, request, baseURL }, testInfo) => {
  const session = await ownAccount(request, baseURL!, testInfo.project.name);
  await rpc(request, baseURL!, session.token, "import_pack", { request: { fileName: "refresh-c.xml", contentBase64: PACK } });
  await signInAs(page, session);
  await openContent(page, testInfo);
  await showsPacks(page, 1);

  // A pack imported from another device.
  let start = Date.now();
  await rpc(request, baseURL!, session.token, "import_pack", { request: { fileName: "refresh-d.xml", contentBase64: PACK } });
  const answered = Date.now() - start;
  let ms = await timeUntil(async () => (await packCount(page)) === 2);
  testInfo.annotations.push({ type: "timing", description: `pack added elsewhere: shown ${ms} ms after the server answered (${answered} ms)` });
  expect(ms, "the added pack showed").toBeLessThan(SOON_MS);

  // An index file added and removed from another device: no content reload, the list follows all the same.
  const indexes = page.getByRole("list", { name: "Indexes" });
  const indexRow = indexes.getByRole("listitem").filter({ hasText: INDEX });
  await rpc(request, baseURL!, session.token, "add_index", { url: INDEX });
  ms = await timeUntil(async () => (await indexRow.count()) === 1);
  testInfo.annotations.push({ type: "timing", description: `index added elsewhere: shown after ${ms} ms` });
  expect(ms, "the added index showed").toBeLessThan(SOON_MS);

  // Removed here: it leaves at once (no pack came from it, so nothing is asked).
  await indexRow.scrollIntoViewIfNeeded();
  await indexRow.getByRole("button", { name: "Remove" }).click();
  ms = await timeUntil(async () => (await indexRow.count()) === 0);
  testInfo.annotations.push({ type: "timing", description: `index removed here: gone after ${ms} ms` });
  expect(ms, "the removed index left the list").toBeLessThan(SOON_MS);
  await expect.poll(async () => ((await rpc(request, baseURL!, session.token, "list_indexes", {})) as unknown[]).length).toBe(0);

  // A pack removed from another device.
  const packs = ((await rpc(request, baseURL!, session.token, "list_packs", {})) as { id: string; kind: string }[]).filter((p) => p.kind !== "bundled");
  start = Date.now();
  await rpc(request, baseURL!, session.token, "remove_pack", { packId: packs[0]!.id });
  ms = await timeUntil(async () => (await packCount(page)) === 1);
  testInfo.annotations.push({ type: "timing", description: `pack removed elsewhere: gone ${ms} ms after the server answered (${Date.now() - start - ms} ms)` });
  expect(ms, "the removed pack left the list").toBeLessThan(SOON_MS);
  expect(await notReloaded(page), "the page reloaded").toBe(true);
});
