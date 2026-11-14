import type { Page, TestInfo } from "@playwright/test";
import { join } from "node:path";
import { expect, measureOverflow, minTap, open, smallTargets, test } from "./fixtures";

/**
 * Account → AI apps (0.3.2) at 1440x900 (desktop-wide) and 384x832 (android-portrait):
 * - since 0.3.3 the only AI: connecting ChatGPT or Claude comes first, then the instructions, then keys, and the
 *   page says Borealis Party never calls an AI service itself; there is no "Your AI" card any more;
 * - the connection guides in tabs (ChatGPT, Claude, Developer tools), with numbered steps and example prompts;
 * - the keys in their own collapsible section;
 * - the instructions for the assistant, read-only with Copy, as the real server sends them.
 * The rig's server has no public address, so `GET /api/me/ai` is given one in the browser (page.route), as a server
 * with `server.public_url` answers. Each view fits: no sideways scroll, full-size tap targets on the phone.
 */

const MCP = "https://party.example.com/api/mcp";

async function fits(page: Page, testInfo: TestInfo, within: string) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  if (testInfo.project.name === "android-portrait") {
    const small = (await smallTargets(page, minTap(testInfo), within)).map((t) => `${t.label} ${t.width}x${t.height}`);
    expect(small, `small controls in ${within}`).toEqual([]);
  }
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-ai-guides-${name}.png`) });
}

test("AI apps: connection guides, keys and the assistant's instructions", async ({ page, signIn }, testInfo) => {
  test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "1440x900 and 384x832");
  await page.route("**/api/me/ai", async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: { ...body, mcpUrl: MCP, oauth: true } });
  });
  await signIn("player");
  if (testInfo.project.name === "android-portrait") await open(page, "/account");
  else await page.goto("/#/account");
  const card = page.locator("section.ai-apps");
  await expect(card).toBeVisible({ timeout: 30_000 });
  await expect(card.getByRole("textbox", { name: "MCP server address" })).toHaveValue(MCP);

  const tabs = card.getByRole("tablist", { name: "Connection guides" });
  await tabs.scrollIntoViewIfNeeded();
  const panel = card.getByRole("tabpanel");
  await expect(panel).toContainText("Add custom MCP server");
  await expect(panel).toContainText("Build a forest ambush for my level 5 party from my bestiary.");
  await fits(page, testInfo, ".ai-guides");
  await shot(page, testInfo, "chatgpt");

  await tabs.getByRole("tab", { name: "Claude" }).click();
  await expect(panel).toContainText("Add custom connector");
  await fits(page, testInfo, ".ai-guides");
  await tabs.getByRole("tab", { name: "Developer tools" }).click();
  await expect(panel).toContainText(`claude mcp add --transport http borealis-party ${MCP}`);
  await fits(page, testInfo, ".ai-guides");
  await shot(page, testInfo, "dev");

  const keys = card.locator("details.ai-keys");
  await expect(keys.getByRole("button", { name: "New token" })).toBeHidden();
  await keys.locator("summary").click();
  await expect(keys.getByRole("button", { name: "New token" })).toBeVisible();

  const box = card.getByRole("textbox", { name: "Instructions for the assistant" });
  await box.scrollIntoViewIfNeeded();
  await expect(box).toHaveJSProperty("readOnly", true);
  const text = await box.inputValue();
  expect(text).toContain("Dungeon Master's assistant");
  expect(text).toContain("homebrew_validate");
  await expect(card.getByRole("button", { name: "Copy instructions" })).toBeVisible();
  await fits(page, testInfo, ".ai-instructions");
  await shot(page, testInfo, "instructions");
});

test("AI apps comes first as the only AI: guides, then instructions, then keys; no Your AI (0.3.3)", async ({ page, signIn }, testInfo) => {
  test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "1440x900 and 384x832");
  await page.route("**/api/me/ai", async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: { ...body, mcpUrl: MCP, oauth: true } });
  });
  // The removed assistant's routes are gone from the server.
  const old = await page.request.get("/api/ai");
  expect(old.status()).toBe(404);
  await signIn("player");
  if (testInfo.project.name === "android-portrait") await open(page, "/account");
  else await page.goto("/#/account");
  const card = page.locator("section.ai-apps");
  await expect(card).toBeVisible({ timeout: 30_000 });
  await expect(card).toContainText("Borealis Party uses your own ChatGPT or Claude");
  await expect(card).toContainText("Borealis Party never calls an AI service itself.");
  await expect(page.getByRole("heading", { name: "Your AI" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Connect OpenRouter" })).toHaveCount(0);

  const top = async (locator: ReturnType<typeof card.locator>) => (await locator.boundingBox())!.y;
  const guides = await top(card.getByRole("heading", { name: "Connect ChatGPT or Claude" }));
  const instructions = await top(card.getByRole("heading", { name: "Instructions for the assistant" }));
  const keys = await top(card.locator("details.ai-keys > summary"));
  expect(guides, "the guides come before the instructions").toBeLessThan(instructions);
  expect(instructions, "the instructions come before the keys").toBeLessThan(keys);
  await fits(page, testInfo, "section.ai-apps");
  await card.getByRole("heading", { name: "Connect ChatGPT or Claude" }).scrollIntoViewIfNeeded();
  await shot(page, testInfo, "order");
});
