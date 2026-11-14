import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Frame, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, signedInShell, test } from "./fixtures";

/**
 * Sheet page frame on a page that grows (fix/sheet-heading-bars). A page whose text needs more room than its paper
 * grows, and the frame art was stretched with it as a whole: its top rules, scaled about ten times, ran across both
 * columns as two thick dark bars, through the "Class features" and "Appearance" headings and the first row under them.
 * The frame now comes in three slices and only the middle one (the side rules) stretches. This loads a character
 * whose story makes a page several papers tall on screen, and checks at a DM's 1440x900 screen and at the sheet's print
 * size (print media: the pages at their paper width, each kept at one sheet) that:
 *  - the frame's top and bottom slices keep the paper's scale and sit at the page's top and bottom edges;
 *  - every heading's rule is a thin line inside its heading, and no heading's box overlaps the text under it.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
const STORY = "I kept a careful list of every promise I made and every one I broke, and I read it aloud whenever the night was quiet enough to hear myself think.";
/** Frame slice height and heading rule height in pt (html_sheet.rs FRAME_EDGE, html_sheet_css.rs `h2 svg.hr`). */
const EDGE_PT = 40;
const RULE_PT = 12;

async function rpc<T>(request: APIRequestContext, auth: string, command: string, data: unknown): Promise<T> {
  const r = await request.post(`/api/rpc/${command}`, { headers: { authorization: `Bearer ${auth}` }, data });
  const text = await r.text();
  expect(r.ok(), `${command}: ${r.status()} ${text.slice(0, 200)}`).toBeTruthy();
  return (text ? JSON.parse(text) : null) as T;
}

/** The player's copy of the test hero with a story long enough to make its page several papers tall. */
async function tallPageHero(request: APIRequestContext): Promise<{ id: string; name: string }> {
  const login = await request.post("/api/auth/login", { data: { username: "player", password: ACCOUNTS.player.password } });
  const auth = ((await login.json()) as { token: string }).token;
  const report = await rpc<{ summary: { id: string; name: string } }>(request, auth, "import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
  const id = report.summary.id;
  await rpc(request, auth, "load_character", { characterId: id });
  await rpc(request, auth, "update_details", {
    characterId: id,
    patch: {
      backstory: Array(40).fill(STORY).join("\n\n"),
      organization: "The wardens of the northern lighthouse.",
      allies: `${STORY} ${STORY}`,
      notesLeft: `${STORY} ${STORY}`,
    },
  });
  await rpc(request, auth, "save_character", { characterId: id });
  await rpc(request, auth, "close_character", { characterId: id, discard: false });
  return { id, name: report.summary.name };
}

async function previewFrame(page: Page): Promise<Frame> {
  await expect(page.locator('iframe[title="Character sheet preview"]')).toBeVisible({ timeout: 60_000 });
  let frame: Frame | undefined;
  await expect.poll(async () => {
    frame = page.frames().find((f) => f.url() === "about:srcdoc");
    return frame ? await frame.locator(".page").count() : 0;
  }, { timeout: 60_000 }).toBeGreaterThan(1);
  return frame!;
}

type Probe = { tallest: number; paperRatio: number; twoColumnHeadings: number; problems: string[] };

/** Runs in the sheet frame: measure every page's frame slices and heading rules. */
function probe([edgePt, rulePt]: [number, number]): Probe {
  const problems: string[] = [];
  const near = (a: number, b: number, tol = 1.5) => Math.abs(a - b) <= tol;
  const pages = [...document.querySelectorAll<HTMLElement>(".page")];
  let tallest = 0;
  let paperRatio = 0;
  let twoColumnHeadings = 0;
  pages.forEach((p, i) => {
    const pb = p.getBoundingClientRect();
    const paperPt = p.classList.contains("a4") ? [595, 842] : [612, 792];
    const px = pb.width / paperPt[0]; // CSS px per pt at this scale
    paperRatio = paperPt[1] / paperPt[0];
    if (pb.height / pb.width > tallest) tallest = pb.height / pb.width;
    const at = `page ${i + 1} (${p.dataset.page})`;
    const top = p.querySelector("svg.frame.f-top");
    const bot = p.querySelector("svg.frame.f-bot");
    const mid = p.querySelector("svg.frame.f-mid");
    if (!top || !bot || !mid) {
      const stretch = pb.height / (paperPt[1] * px);
      problems.push(`${at}: frame is one SVG over the whole page, stretched ${stretch.toFixed(2)}x vertically`);
      return;
    }
    const tb = top.getBoundingClientRect();
    const bb = bot.getBoundingClientRect();
    if (!near(tb.height, edgePt * px) || !near(bb.height, edgePt * px)) problems.push(`${at}: frame top/bottom slice is ${tb.height.toFixed(1)}/${bb.height.toFixed(1)} px, not ${(edgePt * px).toFixed(1)} (stretched)`);
    if (!near(tb.top, pb.top) || !near(bb.bottom, pb.bottom)) problems.push(`${at}: frame top/bottom slice is not at the page edge`);
    const headings = [...p.querySelectorAll<HTMLElement>(".content h2")];
    for (const h of headings) {
      const hb = h.getBoundingClientRect();
      if (hb.top < tb.bottom - 0.5) problems.push(`${at}: heading "${h.textContent?.trim()}" is under the frame's top rules`);
      const rule = h.querySelector("svg.hr");
      if (rule) {
        const rb = rule.getBoundingClientRect();
        if (rb.height > rulePt * px + 1) problems.push(`${at}: rule of "${h.textContent?.trim()}" is ${rb.height.toFixed(1)} px tall`);
        if (rb.left < hb.left - 0.5 || rb.right > hb.right + 0.5 || rb.top < hb.top - 0.5 || rb.bottom > hb.bottom + 0.5) problems.push(`${at}: rule of "${h.textContent?.trim()}" leaves its heading`);
      }
      // The heading's box ends before the next element's text starts.
      const next = h.nextElementSibling;
      if (next && next.getBoundingClientRect().height && next.getBoundingClientRect().top < hb.bottom - 0.5) problems.push(`${at}: heading "${h.textContent?.trim()}" overlaps what follows it`);
    }
    // Headings side by side (Class features | Appearance on a 2014 Details page).
    for (const a of headings) for (const b of headings) {
      const ab = a.getBoundingClientRect();
      const bb2 = b.getBoundingClientRect();
      if (a !== b && near(ab.top, bb2.top, 2) && ab.right <= bb2.left) twoColumnHeadings++;
    }
  });
  return { tallest, paperRatio, twoColumnHeadings, problems };
}

test.describe("Sheet frame on a page that grows", () => {
  for (const mode of ["screen 1440x900", "print size"] as const) {
    test(`top and bottom rules stay at the page edges, heading rules stay thin (${mode})`, async ({ page, signIn, request }, testInfo) => {
      test.skip(testInfo.project.name !== "desktop-wide", "a DM's big screen");
      const hero = await tallPageHero(request);
      await signIn("player");
      await page.goto("/#/characters");
      await signedInShell(page);
      // Desktop: a double-click on the tile loads the character.
      await page.getByRole("button", { name: new RegExp(`^${hero.name},`) }).first().dblclick();
      await expect(page).not.toHaveURL(/#\/characters/, { timeout: 30_000 });
      await expect(page.locator("html")).not.toHaveClass(/\bloading\b/);
      await page.waitForTimeout(1000);
      await page.evaluate(() => { location.hash = "#/sheet?preview=1"; });
      const frame = await previewFrame(page);
      // Print media drops the preview's zoom: the pages are laid out at their paper width (612pt / 595pt).
      if (mode === "print size") {
        await page.emulateMedia({ media: "print" });
        await page.waitForTimeout(500);
      }
      const found = await frame.evaluate(probe, [EDGE_PT, RULE_PT] as [number, number]);
      testInfo.annotations.push({ type: "measured", description: `tallest page ${found.tallest.toFixed(2)} x its width (paper ${found.paperRatio.toFixed(2)})` });
      // Show the grown page's top (the headings under the frame's top rules) in the screenshot.
      await frame.evaluate(() => {
        const tall = [...document.querySelectorAll(".page")].find((p) => p.querySelector("h2")?.closest(".page") && p.textContent?.includes("Backstory"));
        tall?.scrollIntoView({ block: "start" });
      });
      await page.screenshot({ path: testInfo.outputPath(`sheet-frame-${mode === "print size" ? "print" : "1440"}.png`) });
      expect(found.problems, "frame rules or heading rules out of place").toEqual([]);
      expect(found.twoColumnHeadings, "headings side by side in two columns").toBeGreaterThan(0);
      // On screen the long story makes its page grow. Print keeps every page at one sheet (the renderer's print CSS),
      // so there the check is that the frame is still drawn right at the paper's own height.
      if (mode === "print size") expect(found.tallest, "print keeps each page at its paper height").toBeCloseTo(found.paperRatio, 2);
      else expect(found.tallest, "a page grew well past its paper").toBeGreaterThan(found.paperRatio * 1.5);
      await page.emulateMedia({ media: null });
    });
  }
});
