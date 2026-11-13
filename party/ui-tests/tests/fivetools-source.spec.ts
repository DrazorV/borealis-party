import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import type { Locator, Page, Route, TestInfo } from "@playwright/test";
import { expect, measureOverflow, minTap, open, scrollIntoReach, signedInShell, smallTargets, test } from "./fixtures";

/**
 * The Import screen's 5etools card: two sources to choose from, the default one in one tap and the account's own
 * address. Nothing here reaches GitHub:
 * - the default source's download is stubbed in the browser (the job, the scan and the import answer canned data,
 *   with a report as long as a real full import's: hundreds of entries in groups);
 * - an own source is a 5etools-format folder on a local web server, which the server refuses to download from (a
 *   loopback address), and the screen shows that refusal in the server's words.
 */

const DEFAULT_URL = "https://github.com/5etools-mirror-3/5etools-src";

/** A 5etools-format data folder (books.json and one book), as someone's own mirror would serve it. */
let dataServer: Server;
let dataUrl = "";
test.beforeAll(async () => {
  const files: Record<string, string> = {
    "/data/books.json": JSON.stringify({ book: [{ name: "SRD Sample", id: "SRDX", source: "SRDX" }] }),
    "/data/spells/index.json": JSON.stringify({ SRDX: "spells-srdx.json" }),
    "/data/spells/spells-srdx.json": JSON.stringify({ spell: [{ name: "Spark", source: "SRDX", level: 0 }] }),
  };
  dataServer = createServer((req, res) => {
    const body = files[(req.url ?? "").split("?")[0]!];
    res.writeHead(body ? 200 : 404, { "content-type": "application/json" });
    res.end(body ?? "{}");
  });
  await new Promise<void>((done) => dataServer.listen(0, "127.0.0.1", done));
  dataUrl = `https://127.0.0.1:${(dataServer.address() as AddressInfo).port}/data/`;
});
test.afterAll(async () => { await new Promise((done) => dataServer.close(done)); });

const results = (page: Page) => page.getByRole("list", { name: "Import results" });
const row = (page: Page, title: string) => results(page).locator("li.party-import-row").filter({ has: page.locator(".party-import-name", { hasText: title }) });
const card = (page: Page) => page.locator("#party-import-5etools");

/** Upstream's content notice, shown until the account accepts it once (other tests may have). */
async function acceptNotice(page: Page, next: Locator) {
  const notice = page.getByRole("dialog", { name: "Import only content you own" });
  const first = await Promise.race([
    next.waitFor({ state: "visible", timeout: 30_000 }).then(() => "next" as const),
    notice.waitFor({ state: "visible", timeout: 30_000 }).then(() => "notice" as const),
  ]);
  if (first === "notice") await notice.getByRole("button", { name: "I understand, continue" }).tap();
}

/** The first download from a host asks first (upstream's wording). */
async function allowDownload(page: Page) {
  const consent = page.getByRole("dialog", { name: "Add content source" });
  await acceptNotice(page, consent);
  await expect(consent).toBeVisible();
  await consent.getByRole("button", { name: "Download" }).tap();
}

async function expectPhoneLayout(page: Page, testInfo: TestInfo) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), ".party-import")).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `Import screen controls under ${minTap(testInfo)} px`).toEqual([]);
}

/** A report as long as a full import's: 833 entries in a handful of groups. */
function bigReport() {
  const note = (i: number, reason: string) => ({ element: `Entry ${i}`, kind: "spell", reason });
  const partial = Array.from({ length: 520 }, (_, i) => note(i, i % 3 === 0 ? `Uses table "T${i}" that is not supported` : `Option ${i} could not be converted`));
  const skipped = Array.from({ length: 313 }, (_, i) => note(i, i % 2 === 0 ? "Vehicles are not imported" : `Needs source "X${i}"`));
  return { converted: 4210, replaced: 0, partial, skipped, packs: [{ id: "p1", name: "SRD Sample" }, { id: "p2", name: "More" }] };
}

/** The default source's download, scan and import answered in the browser; the rest goes to the real server. */
async function stubDefaultSource(page: Page) {
  const seen: string[] = [];
  let imported = false;
  const json = (route: Route, body: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(/\/api\/rpc\/(job_start|job_status|scan_5etools_folder|import_5etools|list_5etools_urls)$/, async (route) => {
    const name = route.request().url().split("/").pop()!;
    const args = route.request().postDataJSON() as Record<string, unknown> | null;
    if (name === "job_start") {
      if (args?.command !== "stage_5etools_url") return route.continue();
      seen.push(`stage ${(args.args as { url: string }).url}`);
      return json(route, { jobId: "a".repeat(32), command: "stage_5etools_url", started: true, same: true });
    }
    if (name === "job_status") {
      if (args?.jobId !== "a".repeat(32)) return route.continue();
      return json(route, { jobId: "a".repeat(32), command: "stage_5etools_url", state: "done", result: { token: "stub", name: DEFAULT_URL, path: DEFAULT_URL } });
    }
    if (name === "scan_5etools_folder") {
      if (args?.folder !== "stub") return route.continue();
      return json(route, { sources: [{ abbr: "SRDX", name: "SRD Sample", ruleset: "dnd2014", category: "book", counts: { spell: 300 }, unsupported: {}, needs: [] }] });
    }
    if (name === "import_5etools") {
      seen.push("import");
      imported = true;
      return json(route, bigReport());
    }
    // list_5etools_urls: the default source shows as imported once it was.
    return json(route, imported ? [{ url: DEFAULT_URL, revision: "abc123", sources: ["SRDX"], packs: ["SRD Sample", "More"], lastChecked: null, lastImported: null, problem: null }] : []);
  });
  return seen;
}

test.describe("Import screen: 5etools sources", () => {
  test.beforeEach(async ({ signIn }) => {
    await signIn("player");
  });

  test("shows both sources, the default one named with its address", async ({ page }, testInfo) => {
    await open(page, "/import-files");
    await signedInShell(page);
    const def = page.getByRole("region", { name: "Default 5etools data" });
    const own = page.getByRole("region", { name: "My own 5etools source" });
    await expect(def.getByText(DEFAULT_URL)).toBeVisible();
    await expect(def.getByRole("button", { name: "Get the default 5etools data" })).toBeVisible();
    await expect(own.getByLabel("Address of my 5etools source")).toBeVisible();
    await expect(own.getByText(/GitHub repository of 5etools data/)).toBeVisible();
    // The generic link box is still there.
    await expect(page.getByLabel("Link to import")).toBeVisible();
    await card(page).scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath("5etools-card.png"), fullPage: true });
    await testInfo.attach("5etools-card", { path: testInfo.outputPath("5etools-card.png"), contentType: "image/png" });
    await expectPhoneLayout(page, testInfo);
  });

  test("the default source in one tap, and a long report stays usable", async ({ page }, testInfo) => {
    const seen = await stubDefaultSource(page);
    await open(page, "/import-files");
    await signedInShell(page);
    const get = page.getByRole("button", { name: "Get the default 5etools data" });
    expect(await scrollIntoReach(page, get, page.viewportSize()!.height)).toBe(true);
    await get.tap();
    await allowDownload(page);

    const r = row(page, "Default 5etools data");
    await expect(r.getByText(`From ${DEFAULT_URL}`)).toBeVisible();
    const importBooks = r.getByRole("button", { name: "Import 1 book" });
    await expect(importBooks).toBeVisible({ timeout: 30_000 });
    await importBooks.tap();
    await expect(r).toHaveAttribute("data-tone", "warning");
    expect(seen).toEqual([`stage ${DEFAULT_URL}`, "import"]);

    // The summary first, then the groups, folded.
    const report = r.locator(".party-import-report");
    await expect(report.getByText(/4210 elements converted into 2 pack/)).toBeVisible();
    const groups = report.locator(".aur-expander-toggle");
    expect(await groups.count()).toBeGreaterThan(2);
    for (const g of await groups.all()) await expect(g).toHaveAttribute("aria-expanded", "false");
    const summaryTop = (await report.getByText(/elements converted/).boundingBox())!.y;
    const firstGroupTop = (await groups.first().boundingBox())!.y;
    expect(summaryTop).toBeLessThan(firstGroupTop);
    // A group opens on a tap and lists its entries; the box scrolls, the page doesn't go sideways.
    await groups.first().tap();
    await expect(groups.first()).toHaveAttribute("aria-expanded", "true");
    await expect(report.locator(".v4-5et-notes li").first()).toBeVisible();
    const box = (await report.boundingBox())!;
    expect(box.height).toBeLessThanOrEqual(page.viewportSize()!.height * 0.5 + 1);

    // The card now lists where the packs came from, with an Update for that source.
    await expect(card(page).getByRole("button", { name: "Update from Default 5etools data" })).toBeVisible();
    await expect(card(page).getByText("2 packs: SRD Sample, More")).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath("5etools-report.png"), fullPage: true });
    await testInfo.attach("5etools-report", { path: testInfo.outputPath("5etools-report.png"), contentType: "image/png" });
    await expectPhoneLayout(page, testInfo);
  });

  test("an own source: checked before sending, refused by the server in its own words, remembered", async ({ page }, testInfo) => {
    await open(page, "/import-files");
    await signedInShell(page);
    const field = page.getByLabel("Address of my 5etools source");
    const send = page.getByRole("button", { name: "Import from my source" });
    expect(await scrollIntoReach(page, field, page.viewportSize()!.height)).toBe(true);

    // Plain http is turned down on the phone; nothing reaches the server.
    await field.fill(dataUrl.replace("https://", "http://"));
    await send.tap();
    await expect(card(page).getByRole("alert")).toHaveText("Use an https:// address.");
    await expect(results(page)).toHaveCount(0);

    // A loopback address passes the check here; the server refuses it.
    await field.fill(dataUrl);
    await expect(card(page).getByRole("alert")).toHaveCount(0);
    await send.tap();
    await allowDownload(page);
    const r = row(page, dataUrl);
    await expect(r).toHaveAttribute("data-tone", "error", { timeout: 30_000 });
    const message = (await r.locator(".party-import-msg").textContent())?.trim() ?? "";
    expect(message.length).toBeGreaterThan(0);
    expect(message).not.toMatch(/^</);
    expect(message).not.toMatch(/answered \d{3}/);
    expect(message, "the server names the refused address").toContain("127.0.0.1");
    testInfo.annotations.push({ type: "refusal", description: message });
    await expectPhoneLayout(page, testInfo);

    // The address is kept for next time on this device.
    await page.reload();
    await signedInShell(page);
    await expect(page.getByLabel("Address of my 5etools source")).toHaveValue(dataUrl);
  });
});
