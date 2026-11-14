import type { Page } from "@playwright/test";
import { expect, measureOverflow, test, unevenRows } from "./fixtures";
import { ownAccount, rpc, signInAs } from "./ownAccount";

/**
 * The first-run checklist (0.3.6, ui/party/onboarding/): a new account sees "Get started" above its characters, each
 * step ticks itself as the account does it, a step's button opens its page, the DM switch adds the DM's steps, and
 * closing it is an account setting (the server keeps it: no device shows it again).
 * Runs on android-portrait (384x832) and desktop-wide (1440x900), on an account of its own.
 */

const card = (page: Page) => page.getByRole("region", { name: "Get started" });
const step = (page: Page, id: string) => card(page).locator(`[data-step="${id}"]`);

test("a new account's checklist ticks itself, links to each page, and stays closed", async ({ page, request, baseURL }, testInfo) => {
  const phone = testInfo.project.name.startsWith("android");
  const session = await ownAccount(request, baseURL!, "ob", testInfo.project.name);
  const auth = { authorization: `Bearer ${session.token}` };
  // A repeat starts over: the setting back off, no DM switch.
  await rpc(request, baseURL!, session.token, "save_settings", { patch: { onboardingDismissed: false } });
  await request.delete(`${baseURL}/api/me/roles/dm`, { headers: auth });
  await signInAs(page, session);
  const go = (route: string) => page.goto(phone ? `/?mock=mobile#${route}` : `/#${route}`);
  const tap = (l: ReturnType<Page["locator"]>) => (phone ? l.tap() : l.click());

  await go("/characters");
  await expect(card(page)).toBeVisible({ timeout: 30_000 });
  // A player: content, a character, joining a party, and the optional AI app.
  await expect(card(page).locator("[data-step]")).toHaveCount(4);
  await expect(step(page, "party")).toContainText("Join a party");
  await expect(step(page, "ai")).toContainText("(optional)");
  const counted = await card(page).locator(".party-onboarding-count").textContent();
  expect(counted).toMatch(/^\d of 3 done$/);

  // A character made elsewhere ticks its step on the next visit.
  const chars = await rpc<unknown[]>(request, baseURL!, session.token, "list_characters", {});
  if (chars.length === 0) {
    await rpc(request, baseURL!, session.token, "create_character", { request: { name: "Onboarding Hero", ruleset: "dnd2014", startingLevel: 1, gender: "", abilityMethod: "standard_array", options: {} } });
  }
  await page.reload();
  await expect(step(page, "character")).toHaveAttribute("data-done", "true", { timeout: 30_000 });
  await expect(step(page, "character")).toContainText("Done.");
  await expect(step(page, "character").getByRole("button")).toHaveCount(0);

  // Layout: no sideways scroll on a phone; equal-height step boxes in a row on a wide screen.
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  if (!phone) expect(await unevenRows(page, ".party-onboarding")).toEqual([]);
  await testInfo.attach(`onboarding-player-${testInfo.project.name}`, { body: await page.screenshot(), contentType: "image/png" });

  // Each step's button opens its page.
  await tap(step(page, "party").getByRole("button", { name: /Parties/ }));
  await expect(page).toHaveURL(/#\/parties/);
  await go("/characters");
  await tap(step(page, "content").getByRole("button", { name: /Add content/ }));
  await expect(page).toHaveURL(/#\/content/);

  // The DM switch: Create a party and Create an encounter.
  const on = await request.post(`${baseURL}/api/me/roles/dm`, { data: {}, headers: auth });
  expect(on.ok(), await on.text()).toBeTruthy();
  await go("/characters");
  await page.reload();
  await expect(step(page, "encounter")).toBeVisible({ timeout: 30_000 });
  await expect(step(page, "party")).toContainText("Create a party");
  await expect(card(page).locator(".party-onboarding-count")).toHaveText(/ of 4 done$/);
  await testInfo.attach(`onboarding-dm-${testInfo.project.name}`, { body: await page.screenshot(), contentType: "image/png" });

  // Closing it: gone now, after a reload, and kept by the server for the account.
  await tap(card(page).getByRole("button", { name: "Close the checklist" }));
  await expect(card(page)).toHaveCount(0);
  await expect.poll(async () => (await rpc<Record<string, unknown>>(request, baseURL!, session.token, "get_settings", {})).onboardingDismissed).toBe(true);
  await page.evaluate(() => localStorage.removeItem("party.onboardingDismissed"));
  await page.reload();
  await expect(page.locator(".v4-start")).toBeVisible({ timeout: 30_000 });
  await expect(card(page)).toHaveCount(0);
});
