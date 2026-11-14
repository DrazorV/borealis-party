import type { Page, TestInfo } from "@playwright/test";
import { join } from "node:path";
import { expect, measureOverflow, minTap, open, smallTargets, test } from "./fixtures";

/**
 * Browser sign-in for AI providers (0.3.2), Account, Your AI, at 1440x900 (desktop-wide) and 384x832 (android-portrait):
 * - The rig's server has no `[ai] secret_key` and no public address. A regular account sees the plain note (no
 *   environment variable), and "Connect OpenRouter" is disabled; an admin also sees how to switch keys on.
 * - With keys switched on (the status answered in the browser, page.route), Connect OpenRouter asks first in an app
 *   dialog and then opens openrouter.ai (also answered in the browser: nothing real is called).
 * - The Claude hint links to AI apps on the same page.
 * Each view fits: no sideways scroll, and every control a full-size tap target on the phone.
 */

async function fits(page: Page, testInfo: TestInfo) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  if (testInfo.project.name === "android-portrait") {
    const small = (await smallTargets(page, minTap(testInfo), ".ai-signin")).map((t) => `${t.label} ${t.width}x${t.height}`);
    expect(small, "small controls in the sign-in row").toEqual([]);
  }
}

async function shot(page: Page, testInfo: TestInfo, name: string) {
  const dir = process.env.PARTY_SHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${testInfo.project.name}-ai-signin-${name}.png`), fullPage: true });
}

/** `GET /api/ai` as the server answered it, with `change` applied. */
async function patchStatus(page: Page, change: (s: Record<string, unknown>) => void) {
  await page.route("**/api/ai", async (route) => {
    if (route.request().method() !== "GET") return route.fallback();
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    change(body);
    await route.fulfill({ response, json: body });
  });
}

/** The Account page: the phone layer on the phone, the desktop layout at 1440x900. */
async function account(page: Page, testInfo: TestInfo) {
  if (testInfo.project.name === "android-portrait") return open(page, "/account");
  await page.goto("/#/account");
  await expect(page.locator("main").first()).toBeVisible({ timeout: 30_000 });
}

const card = (page: Page) => page.locator("section", { has: page.getByRole("heading", { name: "Your AI" }) });

test.describe("AI browser sign-in (0.3.2)", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(!["desktop-wide", "android-portrait"].includes(testInfo.project.name), "1440x900 and 384x832");
  });

  test("without a server key: a plain note for people, and the sign-in disabled", async ({ page, signIn }, testInfo) => {
    // The rig has no public address; the sign-in is offered as a server with one would.
    await patchStatus(page, (s) => { s.signIn = { openrouter: true }; });
    await signIn("player");
    await account(page, testInfo);
    const you = card(page);
    await expect(you).toBeVisible({ timeout: 30_000 });
    await expect(you).toContainText("Saving cloud AI keys isn't switched on for this server yet; ask its admin.");
    await expect(you).not.toContainText("BOREALIS__AI__SECRET_KEY");
    await expect(you.getByRole("button", { name: "Connect OpenRouter" })).toBeDisabled();
    await expect(you).toContainText("Using Claude? Connect claude.ai or Claude Desktop to Borealis Party instead");
    await you.getByRole("button", { name: "Connect OpenRouter" }).scrollIntoViewIfNeeded();
    await fits(page, testInfo);
    await shot(page, testInfo, "player-no-key");
  });

  test("without a server key, an admin is also told how to switch keys on", async ({ page, signIn }, testInfo) => {
    await patchStatus(page, (s) => { s.signIn = { openrouter: true }; });
    await signIn("admin");
    await account(page, testInfo);
    await expect(card(page)).toContainText("For admins:", { timeout: 30_000 });
    await expect(card(page)).toContainText("BOREALIS__AI__SECRET_KEY");
    await expect(card(page).getByRole("button", { name: "Connect OpenRouter" })).toBeDisabled();
    await fits(page, testInfo);
    await shot(page, testInfo, "admin-no-key");
  });

  test("with keys on: Connect OpenRouter asks first, then opens openrouter.ai; the Claude hint goes to AI apps", async ({ page, signIn }, testInfo) => {
    await patchStatus(page, (s) => { s.keys = { canStore: true }; s.signIn = { openrouter: true }; });
    const auth = "https://openrouter.ai/auth?callback_url=https%3A%2F%2Fparty.test%2Fapi%2Fai%2Foauth%2Fopenrouter%2Fcallback&code_challenge=c&code_challenge_method=S256&state=s";
    await page.route("**/api/ai/oauth/openrouter/start", (route) => route.fulfill({ json: { url: auth } }));
    // The whole context: the popup is a page of its own. Nothing real is reached.
    await page.context().route("https://openrouter.ai/**", (route) => route.fulfill({ contentType: "text/html", body: "<h1>OpenRouter (test)</h1>" }));
    await signIn("player");
    await account(page, testInfo);
    const you = card(page);
    const connect = you.getByRole("button", { name: "Connect OpenRouter" });
    await expect(connect).toBeEnabled({ timeout: 30_000 });
    await connect.scrollIntoViewIfNeeded();
    await fits(page, testInfo);
    await shot(page, testInfo, "keys-on");

    await connect.click();
    const dialog = page.getByRole("dialog", { name: "Connect OpenRouter" });
    await expect(dialog).toContainText("goes to OpenRouter");
    const popup = page.waitForEvent("popup");
    await dialog.getByRole("button", { name: "Continue" }).click();
    const tab = await popup;
    await expect.poll(() => tab.url()).toContain("openrouter.ai/auth");
    await tab.close();
    await expect(you).toContainText("Finish signing in to OpenRouter in your browser");
    await fits(page, testInfo);
    await shot(page, testInfo, "waiting");

    await you.getByRole("link", { name: /Account → AI apps/ }).click();
    await expect(page.locator("section.ai-apps")).toBeInViewport();
  });
});
