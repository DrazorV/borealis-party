import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext } from "@playwright/test";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * The party card's design (0.3.3): one header row (name, edition with Change edition inline, DM badge), one toolbar
 * (Encounters, Invite, Add my character, Rename, Delete on the right), and a character's numbers with View on one row
 * on a wide screen. Shots at 1440x900 (desktop-wide) and 384x832 (android-portrait) go to PARTY_SHOTS.
 */
const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");

async function token(request: APIRequestContext, who: AccountName): Promise<string> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok()).toBeTruthy();
  return ((await r.json()) as { token: string }).token;
}
async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const t = await r.text();
  return (t ? JSON.parse(t) : null) as T;
}

test.describe("The party card", () => {
  test("one header row, one toolbar, numbers and View on one row; wraps at 384 px", async ({ page, signIn, request }, testInfo) => {
    test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "the DM's big screen and the S26");
    const dm = await token(request, "admin");
    const player = await token(request, "player");
    await api(request, dm, "POST", "/api/me/roles/dm", {});
    const name = `Card ${Math.random().toString(36).slice(2, 6)}`;
    const party = await api<{ id: string }>(request, dm, "POST", "/api/parties", { name, ruleset: "dnd2024" });
    try {
      const made = await api<{ code: string }>(request, dm, "POST", `/api/parties/${party.id}/invites`, { kind: "code", uses: 1 });
      const hero = (await api<{ summary: { id: string } }>(request, player, "POST", "/api/rpc/import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER })).summary.id;
      await api(request, player, "POST", "/api/rpc/load_character", { characterId: hero });
      await api(request, player, "POST", "/api/join", { code: made.code, characterId: hero });

      await signIn("admin");
      if (testInfo.project.name === "desktop-wide") await page.goto("/#/parties"); else await open(page, "/parties");
      await signedInShell(page);
      const card = page.getByRole("region", { name });
      await expect(card).toBeVisible({ timeout: 30_000 });
      const head = card.locator(".party-card-head");
      await expect(head.getByRole("heading", { name })).toBeVisible();
      await expect(head.getByRole("button", { name: "Change edition" })).toBeVisible();
      await expect(head.getByText("DM", { exact: true })).toBeVisible();
      const bar = card.getByRole("toolbar", { name: `${name} actions` });
      await expect(bar.locator("> *")).toHaveText(["Encounters", "Invite", "Add my character", "Rename", "Delete"]);
      // The DM sees the numbers outside combat.
      const member = card.locator(".party-member").first();
      await expect(member.locator(".party-stat-hp")).toBeVisible({ timeout: 15_000 });
      const boxes = await bar.locator("> *").evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => ({ top: Math.round(r.top), h: Math.round(r.height) })));
      const heights = new Set(boxes.map((b) => b.h));
      expect(heights.size, "the toolbar's buttons are one height").toBe(1);
      if (testInfo.project.name === "desktop-wide") {
        expect(new Set(boxes.map((b) => b.top)).size, "one toolbar row on a desktop").toBe(1);
        const stats = await member.locator(".party-stats").boundingBox();
        const view = await member.getByRole("link", { name: /^View / }).boundingBox();
        expect(Math.abs((stats!.y + stats!.height / 2) - (view!.y + view!.height / 2)), "View on the chips' row").toBeLessThan(10);
      }
      expect((await measureOverflow(page)).page, "the page scrolls sideways").toBeLessThanOrEqual(0);
      await card.scrollIntoViewIfNeeded();
      const shots = process.env.PARTY_SHOTS;
      if (shots) await card.screenshot({ path: `${shots}/${testInfo.project.name}-party-card.png` });
      if (shots) await page.screenshot({ path: `${shots}/${testInfo.project.name}-party-page.png` });
    } finally {
      await api(request, dm, "DELETE", `/api/parties/${party.id}`).catch(() => undefined);
    }
  });
});
