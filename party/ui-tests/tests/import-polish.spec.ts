import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { expect, measureOverflow, test } from "./fixtures";
import { ownAccount, signInAs } from "./ownAccount";

/**
 * The Import results at a glance (0.3.6): a summary line with a count per outcome first, each row's details folded
 * under Details, the characters' missing content grouped once, and what to do next.
 * Runs on android-portrait (384x832) and desktop-wide (1440x900), on an account of its own.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const results = (page: Page) => page.getByRole("list", { name: "Import results" });
const row = (page: Page, title: string) => results(page).locator("li").filter({ has: page.locator(".party-import-name", { hasText: title }) });

test("import results: summary first, details on request, what next", async ({ page, request, baseURL }, testInfo) => {
  const phone = testInfo.project.name.startsWith("android");
  const session = await ownAccount(request, baseURL!, "ip", testInfo.project.name);
  await signInAs(page, session);
  const tap = (l: ReturnType<Page["locator"]>) => (phone ? l.tap() : l.click());
  await page.goto(phone ? "/?mock=mobile#/content" : "/#/content");
  const choose = page.getByRole("button", { name: "Choose files" });
  await expect(choose).toBeVisible({ timeout: 30_000 });

  const chooser = page.waitForEvent("filechooser");
  await tap(choose);
  await (await chooser).setFiles([
    { name: "Test Hero.dnd5e", mimeType: "application/octet-stream", buffer: readFileSync(join(FIXTURES, "d5-hero.dnd5e")) },
    { name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("not something Borealis reads") },
  ]);

  const hero = row(page, "Test Hero.dnd5e");
  await expect(hero).toHaveAttribute("data-tone", /success|warning/, { timeout: 30_000 });
  await expect(row(page, "notes.txt")).toHaveAttribute("data-tone", "error");

  // The summary: one line, a chip per outcome.
  const summary = page.locator(".party-import-summary");
  await expect(summary.getByRole("status")).toHaveText(/Import finished: 2 items, .*1 failed\.$/);
  await expect(summary.getByLabel("Results by outcome")).toContainText("1 failed");

  // What next: the imported character.
  const next = summary.getByLabel("What next");
  await expect(next.getByRole("button", { name: "Open Characters" })).toBeVisible();
  const warned = (await hero.getAttribute("data-tone")) === "warning";
  if (warned && (await summary.getByRole("group", { name: "Missing content" }).count()) > 0) {
    // The character's missing content, grouped once at the top, folded until asked.
    await expect(next.getByRole("button", { name: "Add the missing content" })).toBeVisible();
    await tap(summary.getByRole("group", { name: "Missing content" }).locator(".aur-expander-toggle"));
    await expect(summary.getByRole("group", { name: "Missing content" })).toContainText("used by");
  }

  // Details stay folded until asked.
  const details = hero.getByRole("button", { name: "Details" });
  if (await details.count()) {
    await expect(hero.locator(".party-import-more")).toHaveCount(0);
    await tap(details);
    await expect(hero.locator(".party-import-more")).toBeVisible();
    await expect(hero.getByRole("button", { name: "Hide details" })).toBeVisible();
  }

  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  await summary.scrollIntoViewIfNeeded();
  await testInfo.attach(`import-polish-${testInfo.project.name}`, { body: await page.screenshot(), contentType: "image/png" });

  await tap(next.getByRole("button", { name: "Open Characters" }));
  await expect(page).toHaveURL(/#\/characters/);
});
