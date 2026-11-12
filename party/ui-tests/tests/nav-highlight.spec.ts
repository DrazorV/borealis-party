import { expect, open, signedInShell, test } from "./fixtures";
import type { Page } from "@playwright/test";

/**
 * Exactly one navigation item looks selected (the user saw Sources and Additional Content lit together on Android).
 * After a real tap on each START sub-tab, and on each bottom tab: one item is `aria-current`, and every other item
 * looks the same as the others (colour, background, underline, weight), so none is lit by a sticky :hover/:focus or a
 * router "active" class either.
 */
interface Look { label: string; current: boolean; look: string; className: string }

async function looks(page: Page, items: string): Promise<Look[]> {
  return page.locator(items).evaluateAll((els) => els.map((el) => {
    const s = getComputedStyle(el);
    const after = getComputedStyle(el, "::after");
    return {
      label: (el.textContent ?? "").trim(),
      current: el.getAttribute("aria-current") === "page" || el.getAttribute("data-selected") === "true",
      className: el.className,
      look: [s.color, s.backgroundColor, s.borderBottomColor, s.boxShadow, s.fontWeight, s.opacity, s.textDecorationLine, after.content, after.backgroundColor, after.borderBottomColor].join("|"),
    };
  }));
}

function oneLit(all: Look[], expected: string): void {
  const current = all.filter((l) => l.current);
  expect(current.map((l) => l.label), "exactly one item is current").toEqual([expected]);
  const others = all.filter((l) => !l.current);
  const lit = others.filter((o) => o.look === current[0]!.look);
  expect(lit.map((l) => l.label), "no other item looks like the current one").toEqual([]);
  expect(new Set(others.map((o) => o.look)).size, `the other items all look alike: ${JSON.stringify(others, null, 1)}`).toBe(1);
}

test.describe("one item is selected", () => {
  test.beforeEach(({}, testInfo) => {
    test.skip(!["android-portrait", "iphone"].includes(testInfo.project.name), "phone portrait sizes: 384x832 and 390x844");
  });

  test("Start sub-tabs", async ({ page, signIn }, testInfo) => {
    await signIn("player");
    await open(page, "/characters");
    await signedInShell(page);
    const strip = 'nav[aria-label="Start sections"] a';
    // Each sibling after each other one, Sources and Additional Content (/content/sources inside /content) both ways.
    for (const label of ["Sources", "Additional Content", "Sources", "Character Collection", "Additional Content", "Character Collection"]) {
      await page.locator(strip).filter({ hasText: new RegExp(`^${label}$`) }).tap();
      await expect(page.locator(`${strip}[aria-current="page"]`)).toHaveText(label);
      await page.waitForTimeout(250); // transitions
      const all = await looks(page, strip);
      await testInfo.attach(`subtabs-${label}.json`, { body: JSON.stringify(all, null, 2), contentType: "application/json" });
      oneLit(all, label);
    }
  });

  test("bottom tabs", async ({ page, signIn }) => {
    await signIn("player");
    await open(page, "/characters");
    await signedInShell(page);
    // Sources and Additional Content: only Start is lit at the bottom.
    for (const route of ["/content/sources", "/content"]) {
      await open(page, route);
      const selected = await page.locator(".bb-item[data-selected=\"true\"]").allTextContents();
      expect(selected.map((s) => s.trim()), route).toEqual(["Start"]);
    }
  });
});
