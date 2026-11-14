import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { APIRequestContext, Page, TestInfo } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, measureOverflow, open, signedInShell, test } from "./fixtures";

/**
 * Offline conflict safety (0.3.6) against the REAL server, on the S26 Ultra phone (384x832, android-portrait) and a
 * 1440x900 desktop (desktop-wide). A browser keeps no offline copy (only the apps do), so this test plays the app's
 * device side of the sync with the same commands the app sends (ui/party/sync/sync.ts), and the page shows the
 * dialog the app shows:
 * - the device goes offline and edits its copy; the server's copy changes meanwhile (another device);
 * - back online, the device's push (`ask`, from the old base) is a conflict: the server writes nothing;
 * - the page asks "Changed in two places" with Keep mine / Use the server's / Keep both, the same size, no sideways
 *   scroll; Keep mine is carried out with `forceOver` (the server copy the user saw), and the device's copy is then the server's.
 * The sync logic itself is covered by ui/party/sync/sync.test.ts. Screenshots go to PARTY_SHOTS when it is set.
 */

const PROJECTS = new Set(["android-portrait", "desktop-wide"]);

async function rpc<T>(request: APIRequestContext, auth: string, command: string, data: object): Promise<T> {
  const r = await request.post(`/api/rpc/${command}`, { headers: { authorization: `Bearer ${auth}` }, data });
  expect(r.ok(), `${command}: ${r.status()} ${await r.text()}`).toBeTruthy();
  return (await r.json()) as T;
}

const decode = (b64: string) => JSON.parse(Buffer.from(b64, "base64").toString("utf8")) as { meta: Record<string, unknown> };
const encode = (value: unknown) => Buffer.from(JSON.stringify(value), "utf8").toString("base64");

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (!dir) return;
  mkdirSync(dir, { recursive: true });
  const size = testInfo.project.name === "desktop-wide" ? "1440" : "384";
  await page.screenshot({ path: join(dir, `sync-conflict-${name}-${size}.png`) });
}

test.describe("sync conflict", () => {
  test.beforeEach(async ({}, testInfo) => {
    test.skip(!PROJECTS.has(testInfo.project.name), "one phone and one desktop");
  });

  test("an offline edit and a server change meet: nothing is overwritten and the user chooses", async ({ page, context, request }, testInfo) => {
    const login = await request.post("/api/auth/login", { data: { username: "player", password: ACCOUNTS.player.password } });
    expect(login.ok()).toBeTruthy();
    const { token, user } = (await login.json()) as { token: string; user: { id: string } };
    const call = <T,>(command: string, data: object = {}) => rpc<T>(request, token, command, data);

    const name = `Conflict ${testInfo.project.name}`;
    const made = await call<{ id: string }>("create_character", { request: { name, ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} } });
    await call("save_character", { characterId: made.id });
    await call("close_character", { characterId: made.id, discard: false });
    // The device's copy, and the server file it was made from.
    const base = await call<{ sha256: string; contentBase64: string }>("party_sync_get", { id: made.id });

    // Offline: the device edits its copy.
    await page.addInitScript(([s]) => {
      if (!sessionStorage.getItem("phone-rig.seeded")) {
        localStorage.setItem("party.session", s);
        sessionStorage.setItem("phone-rig.seeded", "1");
      }
    }, [JSON.stringify({ baseUrl: "", token, user })]);
    if (testInfo.project.name === "desktop-wide") await page.goto("/#/characters");
    else await open(page, "/characters");
    await signedInShell(page);
    await context.setOffline(true);
    await expect(page.locator(".party-offline")).toContainText("Reconnecting…", { timeout: 15_000 });
    const mine = decode(base.contentBase64);
    mine.meta.player = "edited offline";

    // Meanwhile the server's copy changes (another device's edit, made from the same base).
    const theirs = decode(base.contentBase64);
    theirs.meta.player = "edited on another device";
    const other = await call<{ result: string; sha256: string }>("party_sync_push", {
      id: made.id, baseSha: base.sha256, editedAtMs: Date.now(), contentBase64: encode(theirs), ask: true,
    });
    expect(other.result).toBe("applied");

    // Back online: the device's push from the old base is a conflict, and the server keeps its copy untouched.
    await context.setOffline(false);
    await expect(page.locator(".party-offline")).toContainText("Back online", { timeout: 15_000 });
    const stale = await call<{ result: string; sha256: string | null }>("party_sync_push", {
      id: made.id, baseSha: base.sha256, editedAtMs: Date.now() + 60_000, contentBase64: encode(mine), ask: true,
    });
    expect(stale).toMatchObject({ result: "conflict", sha256: other.sha256 });
    const server = await call<{ sha256: string; contentBase64: string }>("party_sync_get", { id: made.id });
    expect(server.sha256).toBe(other.sha256);
    expect(decode(server.contentBase64).meta.player).toBe("edited on another device");

    // The device records the conflict (as sync.ts does) and the page asks.
    await page.evaluate(([uid, id, who, seen]) => {
      localStorage.setItem(`party.mirror.${uid}`, JSON.stringify({
        userId: uid, offsetMs: 0, lastSyncMs: Date.now(), packs: {}, pending: [], problems: {},
        characters: { [id]: { local: "L1", serverSha: "base", localSha: "edited", localMtime: "" } },
        conflicts: { [id]: { name: who, deleted: false, serverSha: seen, localSha: "edited" } },
      }));
    }, [user.id, made.id, name, other.sha256]);
    await page.reload();
    await signedInShell(page);
    const dialog = page.getByRole("dialog", { name: "Changed in two places" });
    await expect(dialog).toBeVisible({ timeout: 15_000 });
    await expect(dialog).toContainText(name);
    const choices = ["Keep mine (overwrite)", "Use the server's", "Keep both"].map((label) => dialog.getByRole("button", { name: label, exact: true }));
    for (const c of choices) await expect(c).toBeVisible();
    const boxes = await Promise.all(choices.map(async (c) => (await c.boundingBox())!));
    for (const b of boxes) {
      expect(Math.round(b.width), "the choices are the same width").toBe(Math.round(boxes[0]!.width));
      expect(Math.round(b.height), "the choices are the same height").toBe(Math.round(boxes[0]!.height));
    }
    expect((await measureOverflow(page)).page, "the dialog doesn't make the page scroll sideways").toBeLessThanOrEqual(1);
    await page.waitForTimeout(500);
    await shot(page, testInfo, "dialog");

    // Keep mine: the choice is kept for the next sync, the dialog goes, and the sync sends the copy over the server copy
    // the user saw (and no other).
    await choices[0]!.click();
    await expect(dialog).toHaveCount(0);
    const chosen = await page.evaluate((uid) => JSON.parse(localStorage.getItem(`party.conflictChoices.${uid}`) ?? "{}") as Record<string, unknown>, user.id);
    // The choice is about the two copies the user saw.
    expect(chosen).toEqual({ [made.id]: { choice: "mine", localSha: "edited", serverSha: other.sha256 } });
    const forced = await call<{ result: string }>("party_sync_push", {
      id: made.id, baseSha: base.sha256, editedAtMs: Date.now(), contentBase64: encode(mine), ask: true, forceOver: other.sha256,
    });
    expect(forced.result).toBe("applied");
    const after = await call<{ contentBase64: string }>("party_sync_get", { id: made.id });
    expect(decode(after.contentBase64).meta.player).toBe("edited offline");

    await page.evaluate((uid) => {
      localStorage.removeItem(`party.mirror.${uid}`);
      localStorage.removeItem(`party.conflictChoices.${uid}`);
    }, user.id);
    await call("delete_character", { characterId: made.id });
  });
});
