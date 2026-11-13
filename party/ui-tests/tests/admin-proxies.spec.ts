import { expect, measureOverflow, minTap, open, signedInShell, smallTargets, test } from "./fixtures";
import type { Page } from "@playwright/test";

/**
 * Admin > Settings > Trusted proxies on a phone: it shows how the server sees this request, trusts the proxy in one
 * tap, refuses a list that trusts everyone, and goes back to config.toml. The rig's browser connects straight to the
 * server, so "the proxy" is 127.0.0.1; trusting it changes nothing for the other specs (no forwarding headers).
 */
async function proxiesCard(page: Page) {
  await open(page, "/admin");
  await signedInShell(page);
  await page.getByRole("tab", { name: "Settings" }).tap();
  const card = page.locator("section.card").filter({ has: page.getByRole("heading", { name: "Trusted proxies" }) });
  await expect(card.getByText("Requests arrive from")).toBeVisible();
  return card;
}

test("Admin: trusted proxies fit the screen", async ({ page, signIn }, testInfo) => {
  await signIn("admin");
  const card = await proxiesCard(page);
  await expect(card.getByText(/no forwarding headers|is trusted, but your request names no client/)).toBeVisible();
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), ".party-admin")).filter((t) => /Trust|^Add$|Remove|config\.toml/.test(t.label));
  expect(small, `proxy buttons under ${minTap(testInfo)} px`).toEqual([]);
  if (process.env.PARTY_SHOTS) await card.screenshot({ path: `${process.env.PARTY_SHOTS}/trusted-proxies-${testInfo.project.name}.png` });
});

test("Admin: type, remove and trust proxies, refuse everyone, back to config.toml", async ({ page, signIn }, testInfo) => {
  // It changes a server-wide setting: one project only, so parallel workers don't race on it.
  test.skip(testInfo.project.name !== "android-portrait", "changes a server-wide setting");
  await signIn("admin");
  const card = await proxiesCard(page);

  const field = card.getByLabel("Add a trusted proxy");
  const add = card.getByRole("button", { name: "Add", exact: true });
  await field.fill("0.0.0.0/0");
  await expect(card.getByText(/would trust every address/)).toBeVisible();
  await expect(add).toBeDisabled();
  // More than one, an address and a range; then one removed.
  await field.fill("10.9.9.9, 172.20.20.0/24");
  await add.tap();
  const list = card.getByRole("list", { name: "Trusted proxies in force" });
  await expect(list.getByRole("listitem")).toHaveCount(2);
  await card.getByRole("button", { name: "Remove 172.20.20.0/24" }).tap();
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await expect(list.getByText("10.9.9.9")).toBeVisible();

  const trust = card.getByRole("button", { name: /^Trust this proxy \(/ });
  await trust.tap();
  await expect(card.getByText(/^Saved; it applies now\. The server sees your request as coming from /)).toBeVisible();
  await expect(card.getByText("Trusted", { exact: true })).toBeVisible();
  await expect(trust).toBeHidden();
  if (process.env.PARTY_SHOTS) await card.screenshot({ path: `${process.env.PARTY_SHOTS}/trusted-proxies-saved-${testInfo.project.name}.png` });

  await card.getByRole("button", { name: "Use config.toml instead" }).tap();
  await expect(trust).toBeVisible();
  await expect(card.getByText(/No proxy is trusted \(the default\)/)).toBeVisible();
});
