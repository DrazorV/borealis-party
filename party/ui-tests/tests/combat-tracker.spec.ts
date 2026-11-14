import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, finding, measureOverflow, minTap, open, signedInShell, smallTargets, test } from "./fixtures";

/**
 * The DM's combat tracker (0.3.0, plan 4.4 and 4.4b): phone first at 384x832, landscape 832x384, an iPhone at 390x844
 * and a desktop window. The layout specs run against the tracker's own mock server (`party.combatMock`, an SRD-only
 * sample fight), so they need no combat routes on the test server. The two-account spec needs the real combat server
 * and skips until the test server has it.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");

/** With PARTY_SHOTS=<folder>, a screenshot for looking at by eye. */
async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-combat-${name}.png`), fullPage: false });
}

const phoneProject = (testInfo: TestInfo) => testInfo.project.name !== "desktop";

async function openMock(page: Page, testInfo: TestInfo) {
  await page.addInitScript(() => { localStorage.setItem("party.combatMock", "1"); });
  if (phoneProject(testInfo)) await open(page, "/combat/c1");
  else {
    await page.goto("/#/combat/c1");
    await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
  }
  await signedInShell(page);
  await expect(page.locator("[data-combatant]").first()).toBeVisible({ timeout: 30_000 });
}

/** Next turn: the thumb bar's on a phone in portrait, the header's otherwise. */
function nextButton(page: Page) {
  return page.locator(".cb-thumb:visible .btn, .cb-top-actions .cb-next:visible").first();
}

test.describe("The DM's combat tracker (0.3.0)", () => {
  test("lays out the fight without sideways scrolling, with full-size controls where the DM taps", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await openMock(page, testInfo);
    await shot(page, testInfo, "setup");

    // Typed table rolls for the creatures, then Start.
    for (const [label, n] of [["Knight", 18], ["Adult Red Dragon", 12], ["Priest", 10], ["Cultist 1", 9], ["Cultist 2", 8], ["Mage", 5]] as const) {
      const box = page.getByLabel(`${label} initiative`, { exact: true });
      await box.fill(String(n));
      await box.press("Enter");
    }
    await expect(page.getByText(/Everyone has an initiative|Waiting for Bram/)).toBeVisible();
    await nextButton(page).click();
    await expect(page.getByText("ROUND 1").first()).toBeVisible();
    await expect(page.locator('[data-combatant="knight"]')).toHaveAttribute("aria-current", "true");
    await shot(page, testInfo, "round1");

    const o = await measureOverflow(page);
    expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
    expect(o.scrollers, "containers that scroll sideways").toEqual([]);

    if (phoneProject(testInfo)) {
      const min = minTap(testInfo);
      for (const area of [".cb-thumb", ".cb-hpc", ".cb-resources", ".cb-roster"]) {
        if (!(await page.locator(area).first().isVisible())) continue;
        const small = (await smallTargets(page, min, area)).map((t) => `${t.label} ${t.width}x${t.height}`);
        expect(small, `controls under ${min} px in ${area}`).toEqual([]);
      }
      const rest = (await smallTargets(page, min, ".cb-page")).map((t) => `${t.label} ${t.width}x${t.height}`);
      if (rest.length) finding(testInfo, `combat tracker: ${rest.length} smaller controls elsewhere: ${rest.slice(0, 6).join(", ")}`);
    }
  });

  test("Next turn moves to the next combatant and, on a phone, brings its card into view (C4, C26)", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await openMock(page, testInfo);
    await page.getByRole("button", { name: "Roll initiatives" }).click();
    await expect(page.locator('[data-combatant="dragon"] input')).not.toHaveValue("");
    await nextButton(page).click();
    await expect(page.getByText("ROUND 1").first()).toBeVisible();
    const first = await page.locator('[data-combatant][aria-current="true"]').getAttribute("data-combatant");
    await nextButton(page).click();
    await expect(page.locator(`[data-combatant="${first}"]`)).not.toHaveAttribute("aria-current", "true");
    const now = await page.locator('[data-combatant][aria-current="true"]').getAttribute("data-combatant");
    const card = page.locator("#cb-card");
    await expect(card).toBeVisible();
    if (testInfo.project.name === "android-portrait" || testInfo.project.name === "iphone") {
      // The card is on screen after the scroll, below the sticky resource panel.
      await expect.poll(async () => {
        const box = await card.boundingBox();
        const vh = page.viewportSize()!.height;
        return Boolean(box && box.y < vh * 0.7 && box.y + 40 > 0);
      }, { message: `the current card (${now}) scrolled into view` }).toBe(true);
      // The resource panel stays on top with its More toggle.
      await expect(page.locator(".cb-resources .cb-more-toggle")).toBeVisible();
      await page.locator(".cb-resources .cb-more-toggle").click();
      await expect(page.getByText("Group limits (per round)")).toBeVisible();
    }
    await shot(page, testInfo, "next-turn");
  });

  test("damage typed or rolled, Use with its reason, and reveal (C6, C14, D20 b)", async ({ page, signIn }, testInfo) => {
    await signIn("admin");
    await openMock(page, testInfo);
    await page.locator('[data-combatant="mage"] .cb-row-main').click();
    const card = page.getByRole("region", { name: "Mage details" });
    await card.getByLabel(/Damage or healing/).fill("2d6+3");
    await card.getByRole("button", { name: "Damage", exact: true }).click();
    await expect(card.getByText(/^\d+\/40 HP$/)).not.toHaveText("40/40 HP");
    await card.getByRole("button", { name: "Reveal to players" }).click();
    await expect(card.getByRole("button", { name: "Hide from players" })).toBeVisible();
    // 0.3.2: every action is a turn button; an attack asks for its target, then waits for the table roll.
    await card.locator('[data-turn-button="dagger"]').click();
    const sheet = page.getByRole("dialog", { name: "Mage: Dagger" });
    await sheet.getByRole("button", { name: "Knight" }).click();
    await sheet.getByRole("button", { name: "Attack" }).click();
    await page.getByRole("dialog", { name: "Dagger · Mage" }).getByRole("button", { name: "Later" }).click();
    await expect(card.locator('[data-turn-button="dagger"]')).toBeDisabled();
    await expect(card.locator(".cb-tb-item", { has: page.locator('[data-turn-button="dagger"]') }).getByText("Action spent")).toBeVisible();
    await shot(page, testInfo, "card");
  });
});

test.describe("Two DM tabs stay current (no stale state)", () => {
  test("a write in one tab shows in the other within a second, with no reload", async ({ page, signIn, context }, testInfo) => {
    test.skip(!["android-portrait", "desktop"].includes(testInfo.project.name), "one phone size and the desktop");
    await signIn("admin");
    await openMock(page, testInfo);
    const other = await context.newPage();
    if (phoneProject(testInfo)) await open(other, "/combat/c1");
    else await other.goto("/#/combat/c1");
    await expect(other.locator("[data-combatant]").first()).toBeVisible({ timeout: 30_000 });
    // A marker on the second tab's window: a reload would lose it.
    await other.evaluate(() => { (window as unknown as { stayed: boolean }).stayed = true; });

    const box = page.getByLabel("Mage initiative", { exact: true });
    await box.fill("17");
    await box.press("Enter");
    const t0 = Date.now();
    await expect(other.getByLabel("Mage initiative", { exact: true })).toHaveValue("17", { timeout: 1_000 });
    expect(Date.now() - t0).toBeLessThan(1_000);

    // And back the other way: damage in the second tab shows in the first.
    await other.locator('[data-combatant="mage"]').getByRole("button", { name: "5 damage to Mage" }).click();
    await expect(page.locator('[data-combatant="mage"] .cb-hp-text')).toContainText("35", { timeout: 1_000 });
    expect(await other.evaluate(() => (window as unknown as { stayed?: boolean }).stayed)).toBe(true);
  });
});

// ---- Two accounts against the real combat server (skips until it is there) ----

async function login(request: APIRequestContext, who: AccountName) {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as { token: string; user: unknown };
}

async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}


test("against the real server: a second DM tab follows within 1 s, and the player's view gets the reveal within 2 s", async ({ page, signIn, request }, testInfo) => {
  test.skip(testInfo.project.name !== "android-portrait", "one phone size");
  const dm = await login(request, "admin");
  const player = await login(request, "player");
  await api(request, dm.token, "POST", "/api/me/roles/dm", {});
  const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: `Fight ${Math.random().toString(36).slice(2, 8)}` });
  const probe = await request.fetch(`/api/parties/${party.id}/combat`, { headers: { authorization: `Bearer ${dm.token}` } });
  const encounters = await request.fetch(`/api/parties/${party.id}/encounters`, { headers: { authorization: `Bearer ${dm.token}` } });
  test.skip(probe.status() === 404 || encounters.status() === 404,
    `the test server has no combat or encounter routes (feat/combat-server): combat ${probe.status()}, encounters ${encounters.status()}`);
  const made = await api<{ code: string }>(request, dm.token, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
  const hero = await api<{ summary: { id: string } }>(request, player.token, "POST", "/api/rpc/import_character", {
    fileName: "d5-hero.dnd5e", contentBase64: readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64"),
  });
  await api(request, player.token, "POST", "/api/join", { code: made.code, characterId: hero.summary.id });
  const encounter = await api<{ id: string }>(request, dm.token, "POST", `/api/parties/${party.id}/encounters`, { name: "Goblins" });
  await api(request, dm.token, "POST", `/api/encounters/${encounter.id}/creatures`, { elementId: "ID_SRD521_MONSTER_GOBLIN_WARRIOR", count: 1 });
  await api(request, dm.token, "PATCH", `/api/encounters/${encounter.id}`, { status: "ready" }).catch(() => null);
  const started = await api<{ id: string; state: { combatants: { id: string; kind: string; label: string; visible: boolean }[] } }>(
    request, dm.token, "POST", `/api/encounters/${encounter.id}/start`, { requestId: `rig-${Date.now()}` });
  const goblin = started.state.combatants.find((c) => c.kind === "creature")!;
  expect(goblin.visible).toBe(false);

  await signIn("admin");
  await open(page, `/combat/${started.id}`);
  const dmTab = await page.context().newPage();
  await open(dmTab, `/combat/${started.id}`);
  const reveal = (p: Page) => p.getByRole("button", { name: `Reveal ${goblin.label} to players` });
  await expect(reveal(page)).toBeVisible({ timeout: 30_000 });
  await expect(reveal(dmTab)).toBeVisible({ timeout: 30_000 });
  // The players' order lists only combatants with an initiative: roll the creatures' in the app first.
  await page.getByRole("button", { name: "Roll initiatives" }).click();
  await expect(page.locator(`[data-combatant="${goblin.id}"] input`)).not.toHaveValue("", { timeout: 5_000 });
  await expect(dmTab.locator(`[data-combatant="${goblin.id}"] input`)).not.toHaveValue("", { timeout: 1_000 });
  const seen = async () => JSON.stringify(await api(request, player.token, "GET", `/api/combats/${started.id}`)).includes(goblin.id);
  expect(await seen(), "a hidden creature isn't in the player's view").toBe(false);

  await reveal(page).click();
  const t0 = Date.now();
  await expect(dmTab.getByRole("button", { name: `Hide ${goblin.label} from players` })).toBeVisible({ timeout: 1_000 });
  await expect.poll(seen, { timeout: 2_000, intervals: [100] }).toBe(true);
  expect(Date.now() - t0).toBeLessThan(2_500);

  // A typed damage in one tab shows in the other, from the server's event, with no reload.
  const hpText = (p: Page) => p.locator(`[data-combatant="${goblin.id}"] .cb-hp-text`);
  const before = (await hpText(page).textContent()) ?? "";
  await page.locator(`[data-combatant="${goblin.id}"]`).getByRole("button", { name: `5 damage to ${goblin.label}` }).click();
  await expect(hpText(page)).not.toHaveText(before);
  const after = (await hpText(page).textContent()) ?? "";
  await expect(hpText(dmTab)).toHaveText(after, { timeout: 1_000 });
});
