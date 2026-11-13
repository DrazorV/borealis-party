import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import type { Locator, Page, Route, TestInfo } from "@playwright/test";
import { expect, measureOverflow, minTap, open, scrollIntoReach, signedInShell, smallTargets, test } from "./fixtures";

/**
 * The Import screen's 5etools card: the server's default source in one tap (when its admin set one) and the account's
 * own address. Nothing here reaches GitHub, and no test sets a real default on the shared rig server:
 * - the server's default is answered in the browser (`/api/import/default-5etools`), and its download is stubbed (the job, the scan and the import answer canned data,
 *   with a report as long as a real full import's: hundreds of entries in groups);
 * - an own source is a 5etools-format folder on a local web server, which the server refuses to download from (a
 *   loopback address), and the screen shows that refusal in the server's words.
 */

const DEFAULT_URL = "https://github.com/example/srd-data";

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

/** A report as long as a full import's: 833 notes, worded as the importer words them, in a handful of groups. */
function bigReport() {
  const notes = (n: number, reason: string) => Array.from({ length: n }, (_, i) => ({ element: `Creature ${i}|SRDX`, kind: "monster", reason }));
  const partial = [
    ...notes(411, "creature section: variant sidebars not imported"),
    ...notes(21, "creature section: mythic actions not imported"),
    ...notes(20, "creature section: the reactions' introduction not imported"),
    ...notes(11, "the challenge rating in a coven not imported"),
    ...notes(10, "creature field: fly speeds not imported"),
    ...notes(355, "character option (Gift) kept as text: no builder choice offers it"),
  ];
  const skipped = notes(5, "creature stat block: the creature type is a choice");
  return { converted: 16746, replaced: 0, partial, skipped, packs: [{ id: "p1", name: "SRD Sample" }, { id: "p2", name: "More" }] };
}

/** The server's default source, as an admin would have set it, answered in the browser. */
async function stubDefault(page: Page) {
  await page.route(/\/api\/import\/default-5etools$/, (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ url: DEFAULT_URL }) }));
}

/** The default source's download, scan and import answered in the browser; the rest goes to the real server. */
async function stubDefaultSource(page: Page) {
  await stubDefault(page);
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

  test("without a default from the server, only an own source is offered", async ({ page }, testInfo) => {
    await open(page, "/import-files");
    await signedInShell(page);
    await expect(page.getByRole("region", { name: "My own 5etools source" })).toBeVisible();
    await expect(card(page).getByText(/comes with no 5etools address/)).toBeVisible();
    await expect(page.getByRole("region", { name: "Default 5etools data" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Get the default 5etools data" })).toHaveCount(0);
    await expectPhoneLayout(page, testInfo);
  });

  test("shows both sources, the server's default named with its address", async ({ page }, testInfo) => {
    await stubDefault(page);
    await open(page, "/import-files");
    await signedInShell(page);
    const def = page.getByRole("region", { name: "Default 5etools data" });
    const own = page.getByRole("region", { name: "My own 5etools source" });
    await expect(def.getByText(DEFAULT_URL)).toBeVisible();
    await expect(def.getByText("Set by your server")).toBeVisible();
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

  test("the default source in one tap; a big import is a success with folded notes", async ({ page }, testInfo) => {
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
    // It worked: a success, with one calm line about the notes and the details folded.
    await expect(r).toHaveAttribute("data-tone", "success");
    expect(seen).toEqual([`stage ${DEFAULT_URL}`, "import"]);
    const report = r.locator(".party-import-report");
    await expect(r.locator(".party-import-msg")).toHaveText("Imported 16746 elements into 2 content packs.");
    await expect(report.getByText("828 details couldn't be carried over. They don't stop anything from working. 5 entries couldn't be read and were left out.", { exact: false })).toBeVisible();
    await expect(report.getByText(/Local import — not redistributable/)).toBeVisible();
    await expect(report.locator(".aur-expander-toggle")).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath("5etools-result.png") });
    await testInfo.attach("5etools-result", { path: testInfo.outputPath("5etools-result.png"), contentType: "image/png" });
    await expectPhoneLayout(page, testInfo);

    // Show details: groups in sentence case with their counts, each folded; one opens to say what it means.
    await report.getByRole("button", { name: "Show details" }).tap();
    const groups = report.locator(".aur-expander-toggle");
    await expect(groups.first()).toHaveText("411 Variant sidebars");
    expect(await groups.count()).toBeGreaterThan(4);
    for (const g of await groups.all()) await expect(g).toHaveAttribute("aria-expanded", "false");
    await groups.first().tap();
    await expect(groups.first()).toHaveAttribute("aria-expanded", "true");
    await expect(report.getByText(/the creature's stats are complete/i)).toBeVisible();
    await expect(report.getByText("Creature 0 (SRDX)").first()).toBeVisible();
    const box = (await report.boundingBox())!;
    expect(box.height).toBeLessThanOrEqual(page.viewportSize()!.height * 0.5 + 1);
    await page.screenshot({ path: testInfo.outputPath("5etools-details.png") });
    await testInfo.attach("5etools-details", { path: testInfo.outputPath("5etools-details.png"), contentType: "image/png" });

    // The card now lists where the packs came from, with an Update for that source.
    await expect(card(page).getByRole("button", { name: "Update from Default 5etools data" })).toBeVisible();
    await expect(card(page).getByText("2 packs: SRD Sample, More")).toBeVisible();
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

test("Admin: the default 5etools source is checked, and refused in the server's words", async ({ page, signIn }, testInfo) => {
  await signIn("admin");
  await open(page, "/admin");
  await signedInShell(page);
  await page.getByRole("tab", { name: "Settings" }).tap();
  const box = page.locator("section.card").filter({ has: page.getByRole("heading", { name: "Default 5etools source" }) });
  await box.scrollIntoViewIfNeeded();
  await expect(box.getByText(/comes with none/)).toBeVisible();
  await expect(box.getByText(/^None: the Import screen offers only/)).toBeVisible();
  const field = box.getByLabel("Default 5etools source address");
  const save = box.getByRole("button", { name: "Save" });

  // Checked on the phone first; nothing is saved (the rig server is shared, so no test saves a real default).
  await field.fill("http://mirror.example.com/data/");
  await save.tap();
  await expect(box.getByRole("alert")).toHaveText("Use an https:// address.");
  // The server refuses a loopback address in its own words.
  await field.fill("https://127.0.0.1/data/");
  await save.tap();
  await expect(box.getByRole("alert")).toContainText("127.0.0.1");
  await expect(box.getByText(/^None: the Import screen offers only/)).toBeVisible();

  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  const small = (await smallTargets(page, minTap(testInfo), ".party-admin")).filter((t) => /Save|Offer none|config\.toml|5etools/.test(t.label));
  expect(small, `controls under ${minTap(testInfo)} px`).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("admin-default-source.png") });
});
