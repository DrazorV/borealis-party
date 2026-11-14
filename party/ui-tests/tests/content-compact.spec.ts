import type { Page, Route, TestInfo } from "@playwright/test";
import { coverage, packs, SOURCE_URL, sources } from "./contentFixture";
import { expect, measureOverflow, minTap, signedInShell, test } from "./fixtures";

/**
 * Additional Content with a large library (0.3.1, ui/party/content/compact.ts): the page reads as summary cards that
 * expand. A 5etools source shows its pack count on one line with its packs folded; the Bestiary is a summary line with
 * its ~120 sources under "Show sources", searchable and filterable, one line each; the pack groups are folded to their
 * counts. What is open and the filter stay after a reload. Run on the phones and a 1440x900 desktop window.
 * The library is made up (./contentFixture.ts) and answered in the browser; the rest is the real server.
 */

const desktop = (testInfo: TestInfo) => testInfo.project.name === "desktop";

async function stubLibrary(page: Page) {
  const json = (route: Route, body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(/\/api\/bestiary\/coverage$/, (route) => json(route, coverage()));
  await page.route(/\/api\/rpc\/list_packs$/, (route) => json(route, packs()));
  await page.route(/\/api\/rpc\/list_5etools_urls$/, (route) => json(route, sources()));
}

async function openContent(page: Page, testInfo: TestInfo) {
  if (desktop(testInfo)) {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/#/content");
  } else {
    await page.goto("/?mock=mobile#/content");
    await expect(page.locator("html.phone")).toHaveCount(1);
  }
  await signedInShell(page);
}

const bestiary = (page: Page) => page.getByRole("region", { name: "Bestiary" });
const showSources = (page: Page) => bestiary(page).getByRole("button", { name: /^(Show|Hide) sources \(122\)$/ });
const knownRow = (page: Page) => page.locator(".party-import-known-row").filter({ hasText: SOURCE_URL });

/** Taps on a phone, clicks on the desktop. */
async function press(target: ReturnType<Page["locator"]>, testInfo: TestInfo) {
  await target.scrollIntoViewIfNeeded();
  if (desktop(testInfo)) await target.click();
  else await target.tap();
}

test.beforeEach(async ({ page, signIn }) => {
  await signIn("player");
  await stubLibrary(page);
});

test("everything long starts folded to a summary line", async ({ page }, testInfo) => {
  await openContent(page, testInfo);

  const known = knownRow(page);
  await expect(known).toContainText("5etools-format data · 120 packs · updated", { timeout: 30_000 });
  await expect(known.getByRole("button", { name: "Show packs" })).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByRole("list", { name: "Packs" })).toHaveCount(0);

  await expect(bestiary(page)).toContainText(/creatures with stat blocks in 122 sources · \d+ partly imported/, { timeout: 30_000 });
  await expect(showSources(page)).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByRole("list", { name: "Bestiary by source" })).toHaveCount(0);

  const group = page.getByRole("button", { name: "5etools-format (120)" });
  await expect(group).toHaveAttribute("aria-expanded", "false");
  await expect(page.getByRole("button", { name: "Remove all 5etools-format packs" })).toBeVisible();

  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath("content-folded.png"), fullPage: true });
});

test("the bestiary's sources: one line each, searched, filtered, and remembered", async ({ page }, testInfo) => {
  await openContent(page, testInfo);
  const toggle = showSources(page);
  await expect(toggle).toBeVisible({ timeout: 30_000 });
  if (desktop(testInfo)) {
    // A keyboard opens it.
    await toggle.focus();
    await page.keyboard.press("Enter");
  } else {
    await press(toggle, testInfo);
    expect((await toggle.boundingBox())!.height, "Show sources tap target").toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
  }
  await expect(toggle).toHaveAttribute("aria-expanded", "true");

  const list = page.getByRole("list", { name: "Bestiary by source" });
  const rows = list.getByRole("listitem");
  await expect(rows).toHaveCount(122);
  // SRD first; names without the format label.
  await expect(rows.nth(0)).toHaveAttribute("aria-label", "SRD 5.1 Monsters");
  await expect(rows.nth(1)).toHaveAttribute("aria-label", "SRD 5.2.1 Monsters");
  await expect(rows.nth(2)).toHaveAttribute("aria-label", "Atlas of Ashvale");
  await expect(list).not.toContainText("5etools-format");

  // One line per source on a desktop: name, counts and status in the row's own bar.
  if (desktop(testInfo)) {
    expect((await rows.nth(2).locator(".aur-expander-head").boundingBox())!.height, "a source's row is one line").toBeLessThanOrEqual(32);
  } else {
    expect((await rows.nth(2).getByRole("button").first().boundingBox())!.height, "source tap target").toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
  }
  await page.screenshot({ path: testInfo.outputPath("bestiary-sources.png") });

  const filters = bestiary(page).getByRole("group", { name: "Filter sources" });
  await press(filters.getByRole("button", { name: /^Partly imported/ }), testInfo);
  await expect(filters.getByRole("button", { name: /^Partly imported/ })).toHaveAttribute("aria-pressed", "true");
  const partly = await rows.count();
  expect(partly).toBeGreaterThan(0);
  expect(partly).toBeLessThan(122);
  await bestiary(page).getByRole("searchbox", { name: "Search sources" }).fill("ashvale");
  await expect(rows.first()).toHaveAttribute("aria-label", /Ashvale$/);
  for (const name of await rows.evaluateAll((els) => els.map((e) => e.getAttribute("aria-label") ?? ""))) expect(name).toMatch(/Ashvale$/);

  // A source's own details are inside its expander.
  const first = rows.first();
  await press(first.getByRole("button").first(), testInfo);
  await expect(first.getByRole("group", { name: "By challenge rating" })).toBeVisible();
  await expect(first.getByText(/with details kept as notes$/)).toBeVisible();

  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath("bestiary-filtered.png") });

  // After a reload: still open, still filtered; the search starts empty.
  await page.reload();
  await signedInShell(page);
  await expect(showSources(page)).toHaveAttribute("aria-expanded", "true", { timeout: 30_000 });
  await expect(filters.getByRole("button", { name: /^Partly imported/ })).toHaveAttribute("aria-pressed", "true");
  await expect(rows).toHaveCount(partly);
});

test("a source's packs open as chips with a search", async ({ page }, testInfo) => {
  await openContent(page, testInfo);
  const known = knownRow(page);
  await press(known.getByRole("button", { name: "Show packs" }), testInfo);
  const chips = page.getByRole("list", { name: "Packs" }).getByRole("listitem");
  await expect(chips).toHaveCount(120);
  await expect(chips.first()).toHaveText("Atlas of Ashvale");
  await known.getByRole("searchbox", { name: "Search packs" }).fill("emberfall");
  await expect(chips).toHaveCount(10);
  if (!desktop(testInfo)) {
    expect((await known.getByRole("button", { name: "Hide packs" }).boundingBox())!.height, "Hide packs tap target").toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
  }

  // The 5etools pack group, opened, has its own search.
  await press(page.getByRole("button", { name: "5etools-format (120)" }), testInfo);
  await page.getByRole("searchbox", { name: "Search 5etools-format packs" }).fill("glimmerdeep");
  await expect(page.getByRole("list", { name: "5etools-format packs" }).getByRole("listitem")).toHaveCount(10);

  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  await page.screenshot({ path: testInfo.outputPath("packs-open.png"), fullPage: true });
});

test("the Assistant section is spaced and lined up like the page's other sections (0.3.2)", async ({ page }, testInfo) => {
  await openContent(page, testInfo);
  const assistant = page.getByRole("region", { name: "Assistant" });
  await expect(assistant).toBeVisible({ timeout: 30_000 });
  await expect(assistant.getByRole("button", { name: "Assistant" })).toBeVisible();
  await expect(bestiary(page)).toContainText(/creatures with stat blocks/, { timeout: 30_000 });
  // Every visible section of the page, as boxes: those stacked over each other keep a section's gap.
  const boxes = await page.locator(".v4-content section.v4-csec").evaluateAll((els) => els
    .map((el) => ({ name: el.getAttribute("aria-label") ?? "", r: el.getBoundingClientRect() }))
    .filter((b) => b.r.width > 0 && b.r.height > 0)
    .map((b) => ({ name: b.name, x: b.r.x, y: b.r.y, w: b.r.width, h: b.r.height })));
  const a = boxes.find((b) => b.name === "Assistant")!;
  expect(a, "the Assistant section").toBeTruthy();
  const left = Math.min(...boxes.map((b) => b.x));
  expect(Math.abs(a.x - left), "the Assistant section starts at the page's left edge, like the others").toBeLessThanOrEqual(1);
  for (const b of boxes) {
    if (b === a) continue;
    const sideBySide = b.x >= a.x + a.w || a.x >= b.x + b.w;
    if (sideBySide) continue;
    const gap = b.y >= a.y ? b.y - (a.y + a.h) : a.y - (b.y + b.h);
    expect(gap, `space between Assistant and ${b.name}`).toBeGreaterThanOrEqual(16);
  }
  await assistant.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath("content-assistant.png") });
});
