import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, measureOverflow, minTap, open, scrollIntoReach, signedInShell, smallTargets, test } from "./fixtures";

/**
 * The Import screen (plan I9): reached from More on a phone, it takes a character, an Aurora pack and a Borealis
 * bundle in one pick, sends each to its importer, and lists one result per file. A pasted index link is either added
 * or refused by the server in its own words.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const file = (name: string, path: string) => ({ name, mimeType: "application/octet-stream", buffer: readFileSync(join(FIXTURES, path)) });

/** An index file on a local web server, as someone's own content host would serve it. */
let indexServer: Server;
let indexUrl = "";
test.beforeAll(async () => {
  const body = readFileSync(join(FIXTURES, "import/sample.index"));
  indexServer = createServer((_req, res) => { res.writeHead(200, { "content-type": "application/xml" }); res.end(body); });
  await new Promise<void>((done) => indexServer.listen(0, "127.0.0.1", done));
  indexUrl = `http://127.0.0.1:${(indexServer.address() as AddressInfo).port}/sample.index`;
});
test.afterAll(async () => { await new Promise((done) => indexServer.close(done)); });

const results = (page: import("@playwright/test").Page) => page.getByRole("list", { name: "Import results" });
const row = (page: import("@playwright/test").Page, title: string) => results(page).locator("li").filter({ has: page.locator(".party-import-name", { hasText: title }) });

/** Upstream's content notice, shown until the account accepts it once (other tests may have). */
async function acceptNotice(page: import("@playwright/test").Page, next: import("@playwright/test").Locator) {
  const notice = page.getByRole("dialog", { name: "Import only content you own" });
  const first = await Promise.race([
    next.waitFor({ state: "visible", timeout: 30_000 }).then(() => "next" as const),
    notice.waitFor({ state: "visible", timeout: 30_000 }).then(() => "notice" as const),
  ]);
  if (first === "notice") await notice.getByRole("button", { name: "I understand, continue" }).tap();
}

async function expectPhoneLayout(page: import("@playwright/test").Page, testInfo: import("@playwright/test").TestInfo) {
  const o = await measureOverflow(page);
  expect(o.page, "the page scrolls sideways").toBeLessThanOrEqual(0);
  expect(o.scrollers, "containers that scroll sideways").toEqual([]);
  const small = (await smallTargets(page, minTap(testInfo), ".party-import")).map((t) => `${t.label} ${t.width}x${t.height}`);
  expect(small, `Import screen controls under ${minTap(testInfo)} px`).toEqual([]);
}

test.describe("Import screen (plan I9)", () => {
  test.beforeEach(async ({ signIn }) => {
    await signIn("player");
  });

  test("More has an Import row that opens the screen", async ({ page }, testInfo) => {
    await open(page, "/more");
    await signedInShell(page);
    const rows = page.getByRole("list", { name: "Account" });
    const importRow = rows.getByRole("link", { name: "Import", exact: true });
    expect(await scrollIntoReach(page, importRow, page.viewportSize()!.height)).toBe(true);
    const box = (await importRow.boundingBox())!;
    expect(box.height).toBeGreaterThanOrEqual(minTap(testInfo) - 0.5);
    await importRow.tap();
    await expect(page.getByRole("button", { name: "Choose files" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Import from my source" })).toBeVisible();
    await expectPhoneLayout(page, testInfo);
  });

  test("one pick: a .dnd5e, an Aurora pack and a .borealis each go to their importer", async ({ page }, testInfo) => {
    await open(page, "/import-files");
    await signedInShell(page);
    const chooser = page.waitForEvent("filechooser");
    const choose = page.getByRole("button", { name: "Choose files" });
    expect(await scrollIntoReach(page, choose, page.viewportSize()!.height)).toBe(true);
    await choose.tap();
    // Names a phone might hand over: the sniffer goes by content.
    await (await chooser).setFiles([
      file("Test Hero.dnd5e", "d5-hero.dnd5e"),
      file("moonfen.xml", "d4-pack.xml"),
      file("m7-hero.borealis", "m7-hero.borealis"),
    ]);

    // The character is imported first, without a dialog.
    await expect(row(page, "Test Hero.dnd5e").getByRole("status")).toHaveText(/^Imported /, { timeout: 30_000 });

    // The pack: the content notice (once per account), then upstream's import preview.
    const preview = page.getByRole("dialog", { name: "Import preview" });
    await acceptNotice(page, preview);
    const install = preview.getByRole("button", { name: "Install" });
    expect(await scrollIntoReach(page, install, page.viewportSize()!.height), "the preview's Install can be reached").toBe(true);
    await install.tap();
    // The pack grants a feat it doesn't have, so upstream's install report comes up; it stays on this screen.
    const report = page.getByRole("dialog", { name: "Import report" });
    const bundle = page.getByRole("dialog", { name: "Open bundle" });
    const next = await Promise.race([
      report.waitFor({ state: "visible", timeout: 30_000 }).then(() => "report" as const),
      bundle.waitFor({ state: "visible", timeout: 30_000 }).then(() => "bundle" as const),
    ]);
    if (next === "report") {
      await expect(bundle, "one dialog at a time").toBeHidden();
      await report.getByRole("button", { name: "Close", exact: true }).last().tap();
    }
    await expect(page.getByRole("button", { name: "Choose files" }), "still on the Import screen").toBeVisible();
    await expect(row(page, "moonfen.xml").getByRole("status")).toHaveText(/^Installed/, { timeout: 30_000 });

    // The bundle: upstream's Open bundle window, after the preview closed.
    await expect(bundle).toBeVisible({ timeout: 30_000 });
    const accept = bundle.getByRole("button", { name: "Accept" });
    expect(await scrollIntoReach(page, accept, page.viewportSize()!.height), "the bundle window's Accept can be reached").toBe(true);
    await accept.tap();
    const bundleRow = row(page, "m7-hero.borealis");
    await expect(bundleRow).toHaveAttribute("data-tone", /success|warning/, { timeout: 30_000 });
    await expect(bundleRow.locator(".party-import-msg")).toHaveText(/^Imported /);

    await expect(results(page).locator("li")).toHaveCount(3);
    await expectPhoneLayout(page, testInfo);
  });

  test("a pasted index link is added, or the server's refusal is shown as it is", async ({ page }, testInfo) => {
    await open(page, "/import-files");
    await signedInShell(page);
    const field = page.getByLabel("Link to import");
    expect(await scrollIntoReach(page, field, page.viewportSize()!.height)).toBe(true);
    await field.fill(indexUrl);
    await page.getByRole("button", { name: "Import", exact: true }).tap();
    // First download from this host: upstream's consent question.
    const consent = page.getByRole("dialog", { name: "Add content source" });
    await expect(consent).toBeVisible();
    await consent.getByRole("button", { name: "Add source" }).tap();

    const r = row(page, indexUrl);
    await expect(r).toHaveAttribute("data-tone", /success|warning|error/, { timeout: 30_000 });
    const tone = await r.getAttribute("data-tone");
    const message = (await r.locator(".party-import-msg").textContent())?.trim() ?? "";
    // This server is plain http on 127.0.0.1: today's check refuses it ("must start with https://"), and the SSRF
    // guard (plan I10) refuses a loopback address too. Either way the row shows the server's words, never a page.
    if (tone === "error") {
      expect(message.length).toBeGreaterThan(0);
      expect(message).not.toMatch(/^</);
      expect(message).not.toMatch(/answered \d{3}/);
      testInfo.annotations.push({ type: "refusal", description: message });
    }
    await expectPhoneLayout(page, testInfo);
  });

  test("an address that could be either asks which it is", async ({ page }, testInfo) => {
    await open(page, "/import-files");
    await signedInShell(page);
    await page.getByLabel("Link to import").fill("https://example.invalid/content");
    await page.getByRole("button", { name: "Import", exact: true }).tap();
    const r = row(page, "https://example.invalid/content");
    await expect(r.getByRole("button", { name: "Add as an index" })).toBeVisible();
    await expect(r.getByRole("button", { name: "Import as 5etools data" })).toBeVisible();
    await expectPhoneLayout(page, testInfo);
  });
});
