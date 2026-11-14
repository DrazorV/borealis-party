import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, open, settled, test } from "./fixtures";

/**
 * The encounter builder's "Add a spell" follows the account's content rules (0.3.5), as its characters do: a book the
 * Sources page restricts (here the Restrict playtest toggle) is never offered, and "also in" names each other book once.
 * The account's own made-up homebrew: "Zephyr Spark" in the book "Zephyr Pages" (marked playtest in the browser's
 * book list, as the bundled books are all official), and three copies of "Ember Mote", two of them in one made-up book. The settings and
 * the server's filtering are the real server's. Run at 1440x900 and on the 384x832 phone, one account per project.
 */

const GOBLIN = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const wide = (testInfo: TestInfo) => testInfo.project.name === "desktop-wide";

async function call<T>(request: APIRequestContext, token: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${token}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

/** This project's own account (`es-<project>`), made on its first run and signed into after. */
async function ownAccount(request: APIRequestContext, project: string): Promise<{ token: string; user: unknown }> {
  const name = `es-${project}`.replace(/[^a-z0-9-]/gi, "").toLowerCase();
  const password = "encounter-sources-test-7731";
  const login = () => request.post("/api/auth/login", { data: { username: name, password } });
  let r = await login();
  if (!r.ok()) {
    let reg = await request.post("/api/auth/register", { data: { username: name, email: `${name}@phone-rig.test`, password } });
    for (let i = 0; i < 3 && reg.status() === 500; i++) {
      await new Promise((done) => setTimeout(done, 500 * (i + 1)));
      reg = await request.post("/api/auth/register", { data: { username: name, email: `${name}@phone-rig.test`, password } });
    }
    expect(reg.ok() || reg.status() === 409, `register: ${reg.status()} ${await reg.text()}`).toBeTruthy();
    const out = (reg.ok() ? await reg.json() : {}) as { token?: string };
    if (!out.token) {
      const admin = (await (await request.post("/api/auth/login", { data: { username: "admin", password: ACCOUNTS.admin.password } })).json()) as { token: string };
      const list = await call<{ items?: { id: string; username: string }[] }>(request, admin.token, "GET", "/api/admin/accounts?perPage=200");
      const id = list.items?.find((a) => a.username === name)?.id;
      await call(request, admin.token, "POST", `/api/admin/accounts/${id}/status`, { status: "active" });
    }
    r = await login();
  }
  expect(r.ok(), `login: ${r.status()} ${await r.text()}`).toBeTruthy();
  return (await r.json()) as { token: string; user: unknown };
}

const set = (name: string, value: string) => ({ name, value });
const spell = (id: string, name: string, source: string, level: string) => ({
  form: { type: "Spell", name, id, source, supports: "Wizard", description: "A made-up spell for the rig.",
    setters: [set("level", level), set("school", "Evocation"), set("time", "1 action"), set("range", "60 feet"), set("duration", "Instantaneous")] },
});

/** The DM's homebrew spells (once per account), a party and an encounter with a goblin. */
async function prepare(request: APIRequestContext, testInfo: TestInfo) {
  const dm = await ownAccount(request, testInfo.project.name);
  await call(request, dm.token, "POST", "/api/me/roles/dm", {});
  await call(request, dm.token, "POST", "/api/rpc/save_settings", { patch: { restrictPlaytest: false, restrictThirdParty: false } });
  const packs = await call<{ id: string; name: string }[] | { packs: { id: string; name: string }[] }>(request, dm.token, "GET", "/api/homebrew/packs");
  const list = Array.isArray(packs) ? packs : packs.packs;
  let pack = list.find((p) => p.name === "Rig Spell Books")?.id;
  if (!pack) {
    pack = (await call<{ id: string }>(request, dm.token, "POST", "/api/homebrew/packs", { name: "Rig Spell Books", description: "Made up for the rig.", author: "Rig" })).id;
    await call(request, dm.token, "POST", `/api/homebrew/packs/${pack}/elements`, spell("ID_RIG_SPELL_ZEPHYR_SPARK", "Zephyr Spark", "Zephyr Pages", "1"));
    await call(request, dm.token, "POST", `/api/homebrew/packs/${pack}/elements`, spell("ID_RIG_SPELL_EMBER_MOTE_A", "Ember Mote", "Made Up Grimoire", "2"));
    await call(request, dm.token, "POST", `/api/homebrew/packs/${pack}/elements`, spell("ID_RIG_SPELL_EMBER_MOTE_B", "Ember Mote", "Made Up Grimoire", "2"));
    await call(request, dm.token, "POST", `/api/homebrew/packs/${pack}/elements`, spell("ID_RIG_SPELL_EMBER_MOTE_C", "Ember Mote", "Second Grimoire", "2"));
  }
  // The DM's pick: the copy in the second book is the spell, the made-up book's two copies are "also in".
  await call(request, dm.token, "POST", "/api/spells/ID_RIG_SPELL_EMBER_MOTE_C/choose");
  const party = await call<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Sources ${Math.random().toString(36).slice(2, 7)}` });
  const enc = await call<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "Spell check", publicName: "A fight" });
  await call(request, dm.token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: GOBLIN, count: 1 });
  return { dm, party, enc };
}

async function press(page: Page, testInfo: TestInfo, locator: ReturnType<Page["locator"]>) {
  if (wide(testInfo)) await locator.click();
  else await locator.tap();
}

/** The builder, the goblin's dialog, and its "Add a spell" picker. */
async function openPicker(page: Page, testInfo: TestInfo, route: string) {
  if (wide(testInfo)) await page.goto(`/#${route}`);
  else await open(page, route);
  const goblin = page.getByRole("listitem", { name: /^Goblin Warrior/ }).first();
  await expect(goblin).toBeVisible({ timeout: 30_000 });
  await press(page, testInfo, goblin.getByRole("button", { name: "Edit" }));
  await press(page, testInfo, page.getByRole("button", { name: "Add spell…" }));
  const picker = page.getByRole("dialog", { name: "Add a spell" });
  await settled(picker);
  return picker;
}

test.beforeEach(async ({}, testInfo) => {
  test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "1440x900 and the 384x832 phone only");
});

test("Add a spell offers only the account's enabled books and names each other book once", async ({ page, request }, testInfo) => {
  const { dm, party, enc } = await prepare(request, testInfo);
  try {
    await page.addInitScript((s) => {
      if (!sessionStorage.getItem("enc-sources.seeded")) {
        localStorage.setItem("party.session", s);
        localStorage.removeItem("borealis.defaultSources");
        sessionStorage.setItem("enc-sources.seeded", "1");
      }
    }, JSON.stringify({ baseUrl: "", ...dm }));
    // "Zephyr Pages" is a playtest book in the browser's book list (made-up content has no Source element).
    await page.route(/\/api\/rpc\/list_sources$/, async (route) => {
      const real = await route.fetch();
      const list = (await real.json()) as { id: string; playtest: boolean }[];
      await route.fulfill({ response: real, json: list.map((b) => (b.id === "Zephyr Pages" ? { ...b, playtest: true } : b)) });
    });
    const route = `/encounters/${party.id}/${enc.id}`;

    // Every book on: the made-up spell is offered, and Ember Mote's other book is named once.
    let picker = await openPicker(page, testInfo, route);
    const search = picker.getByLabel("Search spells");
    await search.fill("zephyr");
    await expect(picker.getByRole("button", { name: /^Zephyr Spark/ })).toBeVisible({ timeout: 15_000 });
    await search.fill("ember");
    const old = picker.getByRole("button", { name: /^Ember Mote/ });
    await expect(old).toBeVisible({ timeout: 15_000 });
    await expect(old).toContainText("also in Made Up Grimoire");
    await expect(old).not.toContainText("Made Up Grimoire, Made Up Grimoire");
    const overflow = await measureOverflow(page);
    expect(overflow.page, "no sideways scroll").toBeLessThanOrEqual(0);
    await page.screenshot({ path: testInfo.outputPath("all-books.png") });

    // Restrict playtest content (as the Sources page saves it): the book's spell is gone after a reload.
    await call(request, dm.token, "POST", "/api/rpc/save_settings", { patch: { restrictPlaytest: true } });
    await page.reload();
    picker = await openPicker(page, testInfo, route);
    await picker.getByLabel("Search spells").fill("zephyr");
    await expect(picker.getByText("No spell in your content matches.")).toBeVisible({ timeout: 15_000 });
    await expect(picker.getByRole("button", { name: /^Zephyr Spark/ })).toHaveCount(0);
    const sent = await page.evaluate(() => performance.getEntriesByType("resource").some((e) => e.name.endsWith("/api/spells/search")));
    expect(sent, "the picker searched through the content rules' route").toBeTruthy();

    // The saved default sources (no toggle): a book left out of them is gone too, and is not named under "also in".
    await call(request, dm.token, "POST", "/api/rpc/save_settings", { patch: { restrictPlaytest: false } });
    const books = await call<{ id: string }[]>(request, dm.token, "POST", "/api/rpc/list_sources", {});
    await page.evaluate((ids) => localStorage.setItem("borealis.defaultSources", JSON.stringify(ids)), books.map((b) => b.id).filter((id) => id !== "Made Up Grimoire"));
    await page.reload();
    picker = await openPicker(page, testInfo, route);
    await picker.getByLabel("Search spells").fill("ember");
    const kept = picker.getByRole("button", { name: /^Ember Mote/ });
    await expect(kept).toBeVisible({ timeout: 15_000 });
    await expect(kept).not.toContainText("also in");
    await page.screenshot({ path: testInfo.outputPath("restricted.png") });
  } finally {
    await call(request, dm.token, "POST", "/api/rpc/save_settings", { patch: { restrictPlaytest: false } }).catch(() => undefined);
    await call(request, dm.token, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
  }
});
