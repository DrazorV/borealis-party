import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { APIRequestContext, Frame, Page } from "@playwright/test";
import { ACCOUNTS } from "./accounts";
import { expect, open, signedInShell, test } from "./fixtures";

/**
 * The character sheet preview on a phone: a character with long text (a backstory over several paragraphs, with
 * Organisation, Allies and Notes after it) opens full-screen, and on no page does a section's text run over the next
 * section or into the page footer ("Borealis character sheet · ... · n / m"). The sheet is upstream's renderer; the
 * preview pages grow instead (fix/sheet-overflow). The Android app's own text size is checked by the Android smoke run
 * (party/android/smoke/run.mjs), which this browser can't imitate.
 */

const FIXTURES = resolve(fileURLToPath(new URL(".", import.meta.url)), "../../../e2e/fixtures");
const CHARACTER = readFileSync(join(FIXTURES, "d5-hero.dnd5e")).toString("base64");
const STORY = "I kept a careful list of every promise I made and every one I broke, and I read it aloud whenever the night was quiet enough to hear myself think.";

async function rpc<T>(request: APIRequestContext, auth: string, command: string, data: unknown): Promise<T> {
  const r = await request.post(`/api/rpc/${command}`, { headers: { authorization: `Bearer ${auth}` }, data });
  const text = await r.text();
  expect(r.ok(), `${command}: ${r.status()} ${text.slice(0, 200)}`).toBeTruthy();
  return (text ? JSON.parse(text) : null) as T;
}

/** The player's copy of the test hero, with long story text; its id and name. */
async function longStoryHero(request: APIRequestContext): Promise<{ id: string; name: string }> {
  const login = await request.post("/api/auth/login", { data: { username: "player", password: ACCOUNTS.player.password } });
  const auth = ((await login.json()) as { token: string }).token;
  const report = await rpc<{ summary: { id: string; name: string } }>(request, auth, "import_character", { fileName: "d5-hero.dnd5e", contentBase64: CHARACTER });
  const id = report.summary.id;
  await rpc(request, auth, "load_character", { characterId: id });
  await rpc(request, auth, "update_details", {
    characterId: id,
    patch: {
      backstory: Array(8).fill(STORY).join("\n\n"),
      organization: "The wardens of the northern lighthouse.",
      allies: `${STORY} ${STORY}`,
      notesLeft: `${STORY} ${STORY}`,
    },
  });
  await rpc(request, auth, "save_character", { characterId: id });
  await rpc(request, auth, "close_character", { characterId: id, discard: false });
  return { id, name: report.summary.name };
}

/** The preview iframe (sandboxed, with the generated sheet), once its pages are there. */
async function previewFrame(page: Page): Promise<Frame> {
  await expect(page.locator('iframe[title="Character sheet preview"]')).toBeVisible({ timeout: 60_000 });
  let frame: Frame | undefined;
  await expect.poll(async () => {
    frame = page.frames().find((f) => f.url() === "about:srcdoc");
    return frame ? await frame.locator(".page").count() : 0;
  }, { timeout: 60_000 }).toBeGreaterThan(1);
  return frame!;
}

test.describe("Sheet preview on a phone", () => {
  test("long story text never runs over the next section or the page footer", async ({ page, signIn, request }, testInfo) => {
    test.skip(testInfo.project.name !== "android-portrait" && testInfo.project.name !== "iphone", "a portrait phone");
    const hero = await longStoryHero(request);
    await signIn("player");
    await open(page, "/characters");
    await signedInShell(page);
    await page.getByTestId(`character-tile-${hero.id}`).click();
    await page.getByRole("button", { name: `Open ${hero.name}` }).click();
    await expect(page.locator("html")).not.toHaveClass(/\bloading\b/);
    await page.waitForTimeout(1000);
    await page.evaluate(() => { location.hash = "#/sheet?preview=1"; });
    const frame = await previewFrame(page);
    const found = await frame.evaluate(() => {
      const problems: string[] = [];
      const name = (el: Element) => `${el.className || el.tagName} "${(el.textContent ?? "").trim().slice(0, 24)}"`;
      const pages = [...document.querySelectorAll(".page")];
      pages.forEach((p, i) => {
        const limit = (p.querySelector(".foot") as Element).getBoundingClientRect().top;
        for (const el of p.querySelectorAll(".content *")) {
          if (el.closest("svg")) continue;
          const b = el.getBoundingClientRect();
          if (b.height && b.bottom > limit + 0.5) { problems.push(`page ${i + 1}: ${name(el)} runs into the footer`); break; }
        }
        for (const sec of p.querySelectorAll(".sec:not(.more-spells)")) {
          if (sec.scrollHeight > sec.clientHeight + 1) problems.push(`page ${i + 1}: ${name(sec)} is shorter than its text`);
        }
      });
      const text = document.body.innerText;
      return { pages: pages.length, problems, story: ["Backstory", "Organisation", "Allies", "Notes"].filter((h) => text.includes(h)) };
    });
    await page.screenshot({ path: testInfo.outputPath("sheet-preview.png") });
    expect(found.story, "the story sections are on the sheet").toEqual(["Backstory", "Organisation", "Allies", "Notes"]);
    expect(found.problems, "sheet text running over a section or the page footer").toEqual([]);
  });
});
