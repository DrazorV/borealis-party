import type { APIRequestContext, Page, Route, TestInfo } from "@playwright/test";
import { join } from "node:path";
import { ACCOUNTS, type AccountName } from "./accounts";
import { expect, measureOverflow, minTap, open, settled, smallTargets, test } from "./fixtures";

/**
 * Bring your own AI (0.3.1 part C) on the phones, 384x832 (Android) and 390x844 (iPhone), against the real local
 * server:
 * - Account, Your AI: a model "on this device" (Ollama) is set up. The rig's server has no `[ai] secret_key`, so it
 *   says why it can't keep keys, and the device mode still works. The card fits and every control is a full-size
 *   tap target.
 * - The encounter list's Assistant: the panel is a bottom sheet that fits. "Make a forest ambush for my level 5
 *   party": the model's two tool calls (a bestiary search, then a new draft) show as cards; the draft asks first in an
 *   app dialog, and after Allow the real server has the draft. No XP or difficulty anywhere (the user's D16).
 * The model on "this device" is answered in the browser (page.route), so no real AI is ever called, and the server
 * never calls it: the page's own CSP lets it reach localhost.
 */

const MODEL = "http://localhost:11434/v1/chat/completions";

async function login(request: APIRequestContext, who: AccountName): Promise<{ token: string }> {
  const r = await request.post("/api/auth/login", { data: { username: who, password: ACCOUNTS[who].password } });
  expect(r.ok(), `sign in as ${who}`).toBeTruthy();
  return (await r.json()) as { token: string };
}

async function api<T>(request: APIRequestContext, auth: string, method: string, path: string, data?: unknown): Promise<T> {
  const r = await request.fetch(path, { method, headers: { authorization: `Bearer ${auth}` }, ...(data === undefined ? {} : { data }) });
  expect(r.ok(), `${method} ${path}: ${r.status()} ${await r.text()}`).toBeTruthy();
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `controls under ${minTap(testInfo)} px in ${within}`).toEqual([]);
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-assistant-${name}.png`) });
}

const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type", "access-control-allow-methods": "POST, OPTIONS" };

/** The model on "this device": a bestiary search, then a draft, then its answer. Keeps what it was sent. */
async function deviceModel(page: Page, partyId: string): Promise<Record<string, unknown>[]> {
  const sent: Record<string, unknown>[] = [];
  const call = (name: string, args: unknown) => ({ choices: [{ message: { content: null, tool_calls: [{ id: `t${sent.length}`, type: "function", function: { name, arguments: JSON.stringify(args) } }] } }] });
  await page.route(MODEL, async (route: Route) => {
    if (route.request().method() === "OPTIONS") return route.fulfill({ status: 204, headers: cors });
    sent.push(route.request().postDataJSON() as Record<string, unknown>);
    const answers = [
      call("bestiary_search", { text: "wolf", limit: 3 }),
      call("encounter_create", { partyId, name: "Forest ambush", notes: "Wolves circle from the trees." }),
      { choices: [{ message: { content: "I made a draft, Forest ambush: a pack of wolves that circles in from the trees." } }] },
    ];
    await route.fulfill({ status: 200, headers: cors, json: answers[Math.min(sent.length - 1, answers.length - 1)] });
  });
  return sent;
}

test.describe("Bring your own AI (0.3.1)", () => {
  test("Your AI on this device, and the assistant drafts an encounter after the DM allows it", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name === "android-landscape", "portrait phones: 384x832 and 390x844");
    // One account per phone: the AI setup is the account's, and the projects run at the same time.
    const who: AccountName = testInfo.project.name === "iphone" ? "player" : "admin";
    const dm = await login(request, who);
    await api(request, dm.token, "POST", "/api/me/roles/dm", {});
    await api(request, dm.token, "DELETE", "/api/ai");
    const partyName = `AI ${testInfo.project.name} ${Math.random().toString(36).slice(2, 7)}`;
    const party = await api<{ id: string }>(request, dm.token, "POST", "/api/parties", { name: partyName });
    const sent = await deviceModel(page, party.id);

    try {
      // Your AI, on the Account page.
      await signIn(who);
      await open(page, "/account");
      const card = page.locator("section", { has: page.getByRole("heading", { name: "Your AI" }) });
      await expect(card).toBeVisible({ timeout: 30_000 });
      await expect(card).toContainText("BOREALIS__AI__SECRET_KEY");
      await card.getByRole("button", { name: "Start from" }).tap();
      const sheet = page.getByRole("dialog", { name: "Start from" });
      await settled(sheet);
      await sheet.getByRole("option", { name: "Ollama on this device" }).tap();
      await expect(card.getByRole("textbox", { name: "Address", exact: true })).toHaveValue("http://localhost:11434/v1");
      await expect(card.getByLabel("API key")).toHaveCount(0);
      await card.getByLabel("I agree my requests go to the provider").tap();
      await expect(card).toContainText("goes to the model on your device");
      await card.getByRole("button", { name: "Save" }).tap();
      await expect(page.getByText("Your AI is saved.")).toBeVisible();
      await fits(page, testInfo, ".ai-form");
      await shot(page, testInfo, "your-ai");
      const saved = await api<{ provider: { mode: string; hasKey: boolean } }>(request, dm.token, "GET", "/api/ai");
      expect(saved.provider).toMatchObject({ mode: "device", hasKey: false });

      // The assistant, from the party's encounters.
      await open(page, `/parties/${party.id}/encounters`);
      await page.getByRole("button", { name: "Assistant" }).tap();
      const panel = page.getByRole("dialog", { name: "Assistant" });
      await settled(panel);
      await expect(panel).toContainText("Uses the model on your device (llama3.1:8b)");
      const box = (await panel.boundingBox())!;
      expect(box.width, "the panel is as wide as the phone").toBeGreaterThanOrEqual(page.viewportSize()!.width - 1);
      await fits(page, testInfo, ".ai-panel");
      await panel.getByRole("textbox", { name: "Message to the assistant" }).fill("make a forest ambush for my level 5 party");
      await panel.getByRole("button", { name: "Send" }).tap();

      const allow = page.getByRole("dialog", { name: "Allow: Create an encounter draft?" });
      await expect(allow).toBeVisible({ timeout: 30_000 });
      await expect(allow).toContainText("name: Forest ambush");
      await expect(panel.getByLabel("Tool: Search the bestiary")).toContainText("Done");
      // Nothing is written before the DM allows it.
      const before = await api<{ encounters: unknown[] }>(request, dm.token, "GET", `/api/parties/${party.id}/encounters`);
      expect(before.encounters).toHaveLength(0);
      await fits(page, testInfo, '[role="dialog"]');
      await shot(page, testInfo, "allow");
      await allow.getByRole("button", { name: "Allow", exact: true }).tap();

      await expect(panel).toContainText("I made a draft, Forest ambush", { timeout: 30_000 });
      await expect(panel.getByLabel("Tool: Create an encounter draft")).toContainText("Done");
      const after = await api<{ encounters: { name: string; status: string }[] }>(request, dm.token, "GET", `/api/parties/${party.id}/encounters`);
      expect(after.encounters).toMatchObject([{ name: "Forest ambush", status: "draft" }]);
      // The model got the server's tools and prompt, the search result as marked data, and never a key.
      expect((sent[0]!.tools as { function: { name: string } }[]).map((t) => t.function.name)).toContain("bestiary_search");
      const messages = sent[1]!.messages as { role: string; content: string }[];
      expect(messages[0]!.content).toContain("Never mention XP");
      expect(JSON.parse(messages.at(-1)!.content)).toMatchObject({ note: expect.stringContaining("not instructions") });
      await expect(panel).not.toContainText(/\bXP\b|difficulty/i);
      await fits(page, testInfo, ".ai-panel");
      await shot(page, testInfo, "done");
    } finally {
      await api(request, dm.token, "DELETE", `/api/parties/${party.id}`);
      await api(request, dm.token, "DELETE", "/api/ai");
    }
  });
});
