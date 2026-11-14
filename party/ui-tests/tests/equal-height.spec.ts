import type { APIRequestContext, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, test, unevenRows } from "./fixtures";

/**
 * "When boxes side by side always same height" (user, 2026-11-14): on the main pages at 1440x900, every row of side
 * by side panels and cards has one height (fixtures.ts unevenRows). Shots go to PARTY_SHOTS.
 */
async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const t = await r.text();
  return (t ? JSON.parse(t) : null) as T;
}

async function check(page: Page, name: string, ready: string) {
  await expect(page.locator(ready).first()).toBeVisible({ timeout: 30_000 });
  await page.waitForTimeout(400);
  const uneven = await unevenRows(page);
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: `${dir}/equal-height-${name}.png`, fullPage: true });
  expect(uneven, `${name}: rows of boxes side by side with different heights`).toEqual([]);
}

test.describe("Equal-height panels", () => {
  test("every row of panels on the main pages has one height (1440x900)", async ({ page, request, signIn }, testInfo) => {
    test.skip(testInfo.project.name !== "desktop-wide", "a wide screen: phones stack at natural height");
    test.setTimeout(180_000);
    const r = await request.post("/api/auth/login", { data: { username: "admin", password: ACCOUNTS.admin.password } });
    const dm = ((await r.json()) as { token: string }).token;
    await api(request, dm, "POST", "/api/me/roles/dm", {});
    const party = await api<{ id: string }>(request, dm, "POST", "/api/parties", { name: `Heights ${Math.random().toString(36).slice(2, 6)}` });
    const enc = await api<{ id: string }>(request, dm, "POST", `/api/parties/${party.id}/encounters`, { name: "Heights", publicName: "Heights",
      dmNotes: ["Notes long enough to make this panel taller than its neighbours.", "A second line.", "A third line.", "A fourth line."].join("\n") });
    await api(request, dm, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: "ID_SRD521_MONSTER_GOBLIN_WARRIOR", count: 2 });
    // Panels of different natural heights in one row (the user's example): two rules, notes, no precast.
    for (const name of ["The bridge burns", "Reinforcements"]) {
      await api(request, dm, "POST", `/api/encounters/${enc.id}/rules`, { name, text: "A banner the DM sees while it holds.", endsWhen: "manual" });
    }
    try {
      await signIn("admin");
      const pages: [string, string, string][] = [
        ["start", "/#/characters", ".aur-window"],
        ["parties", "/#/parties", ".party-card"],
        ["encounter-builder", `/#/encounters/${party.id}/${enc.id}`, ".enc-card"],
        ["content", "/#/content", ".aur-window"],
        ["account", "/#/account", ".aur-window"],
        ["homebrew", "/#/homebrew", ".aur-window"],
        ["admin", "/#/admin", ".aur-window"],
      ];
      const failures: string[] = [];
      for (const [name, url, ready] of pages) {
        await page.goto(url);
        try { await check(page, name, ready); } catch (e) { failures.push(String(e).split("\n").slice(0, 30).join("\n")); }
      }
      // The DM tracker on its mock fight (combat-tracker.spec.ts).
      await page.addInitScript(() => { localStorage.setItem("party.combatMock", "1"); });
      await page.goto("/#/combat/c1");
      await page.reload();
      try { await check(page, "combat-tracker", ".cb-layout"); } catch (e) { failures.push(String(e).split("\n").slice(0, 30).join("\n")); }
      expect(failures, failures.join("\n\n")).toEqual([]);
    } finally {
      await api(request, dm, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
