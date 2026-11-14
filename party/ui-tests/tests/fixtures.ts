import { readFileSync } from "node:fs";
import { test as base, expect, type Page, type TestInfo } from "@playwright/test";
import { ACCOUNTS, SERVER_FILE, type AccountName, type ServerFile } from "./accounts";

export { expect };

/** The phone's smallest comfortable tap target: Apple's 44 pt, Android's 48 dp (plan M3). */
export function minTap(testInfo: TestInfo): number {
  return testInfo.project.name.startsWith("android") ? 48 : 44;
}

/** An on-screen keyboard's height (CSS px) on the test phones, portrait and landscape. */
export function keyboardHeight(page: Page): number {
  const { width, height } = page.viewportSize()!;
  return height > width ? 330 : 200;
}

export const test = base.extend<{ signIn: (who: AccountName) => Promise<void> }>({
  baseURL: async ({}, use) => {
    const { url } = JSON.parse(readFileSync(SERVER_FILE, "utf8")) as ServerFile;
    await use(url);
  },
  /** Signs in through the API and gives the page that session, as the sign-in page would. */
  signIn: async ({ page, request, baseURL }, use) => {
    await use(async (who) => {
      const r = await request.post(`${baseURL}/api/auth/login`, { data: { username: who, password: ACCOUNTS[who].password } });
      expect(r.ok(), `sign in as ${who}: ${r.status()} ${await r.text()}`).toBeTruthy();
      const { token, user } = (await r.json()) as { token: string; user: unknown };
      const session = JSON.stringify({ baseUrl: "", token, user });
      await page.addInitScript((s) => {
        // Only on the first load: a test that signs out must stay signed out.
        if (!sessionStorage.getItem("phone-rig.seeded")) {
          localStorage.setItem("party.session", s);
          sessionStorage.setItem("phone-rig.seeded", "1");
        }
      }, session);
    });
  },
});

/** Open a route of the app with the phone layer on (`?mock=mobile`), and wait for it to draw. */
export async function open(page: Page, route: string): Promise<void> {
  await page.goto(`/?mock=mobile#${route}`);
  await expect(page.locator("html.phone")).toHaveCount(1);
  // The event stream keeps a request open, so "network idle" never comes: wait for the page's own content instead.
  await expect(page.locator("main, .party-connect").first()).toBeVisible({ timeout: 30_000 });
}

/**
 * Wait for upstream's window with the account bar, which a signed-in page always has. On a phone the bar shows only
 * the Sync chip (plan D10), which a browser has none of, so it is in the page but not on screen.
 */
export async function signedInShell(page: Page): Promise<void> {
  await expect(page.locator(".party-bar")).toBeAttached({ timeout: 30_000 });
  await expect(page.locator(".aur-window").first()).toBeVisible({ timeout: 30_000 });
}

/** Record a glitch without failing the test (party/ui-tests/findings.md lists them after the run). */
export function finding(testInfo: TestInfo, text: string): void {
  testInfo.annotations.push({ type: "finding", description: text });
}

export interface Overflow {
  /** How far the page itself scrolls sideways (px). */
  page: number;
  /** Scroll containers (not meant to scroll sideways) that do. */
  scrollers: string[];
  /** Visible elements that reach past the right edge of the screen and are cut off there. */
  clipped: string[];
}

/** What reaches past the screen's width on the current page. */
export function measureOverflow(page: Page): Promise<Overflow> {
  return page.evaluate(() => {
    const vw = document.documentElement.clientWidth;
    const name = (el: Element) => {
      const cls = typeof el.className === "string" && el.className ? `.${el.className.trim().split(/\s+/).slice(0, 3).join(".")}` : "";
      const text = (el.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 40);
      return `${el.tagName.toLowerCase()}${cls}${text ? ` "${text}"` : ""}`;
    };
    const pageScroll = Math.max(document.documentElement.scrollWidth, document.body.scrollWidth) - vw;
    const scrollers: string[] = [];
    const clipped: string[] = [];
    const sideways = (el: Element) => {
      const s = getComputedStyle(el);
      return s.overflowX === "auto" || s.overflowX === "scroll";
    };
    for (const el of Array.from(document.body.querySelectorAll("*"))) {
      const s = getComputedStyle(el);
      if (s.display === "none" || s.visibility === "hidden") continue;
      // A deliberate sideways scroller (tab strips, tables) is fine; one that scrolls only because it overflows isn't.
      if (sideways(el) && el.scrollWidth > el.clientWidth + 1 && s.overflowY !== "visible" && el.clientWidth >= vw - 2
          && !/tab|strip|chips|table|scroll-x|pills|nav/i.test(String((el as HTMLElement).className))) {
        scrollers.push(`${name(el)} scrolls ${el.scrollWidth - el.clientWidth}px sideways`);
      }
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0 || r.right <= vw + 1 || r.left >= vw) continue;
      // Inside a sideways scroller it can be scrolled to.
      let p = el.parentElement;
      let inScroller = false;
      while (p) { if (sideways(p)) { inScroller = true; break; } p = p.parentElement; }
      if (inScroller) continue;
      // Report the outermost clipped element only.
      const parent = el.parentElement?.getBoundingClientRect();
      if (parent && parent.right > vw + 1) continue;
      clipped.push(`${name(el)} reaches ${Math.round(r.right - vw)}px past the right edge`);
    }
    return { page: pageScroll, scrollers, clipped };
  });
}

export interface Target { label: string; width: number; height: number }

/** Report the controls on screen (or inside `within`) smaller than the platform's tap target, as findings. */
export async function reportSmallTargets(page: Page, testInfo: TestInfo, within?: string): Promise<Target[]> {
  const min = minTap(testInfo);
  const small = await smallTargets(page, min, within);
  for (const t of small) finding(testInfo, `tap target ${t.width}x${t.height} px (under ${min}): ${t.label}`);
  return small;
}

/** Visible controls smaller than `min` CSS px on either side, on the page or inside the element `within` selects. */
export function smallTargets(page: Page, min: number, within?: string): Promise<Target[]> {
  return page.evaluate(([min, within]) => {
    const sel = "button, a[href], input:not([type=hidden]), select, textarea, summary, [role=button], [role=tab], [role=menuitem], [role=checkbox], [role=switch], [role=option]";
    const out: { label: string; width: number; height: number }[] = [];
    const scope: ParentNode = (within && document.querySelector(within)) || document;
    for (const el of Array.from(scope.querySelectorAll<HTMLElement>(sel))) {
      let r: { width: number; height: number; left: number; right: number } = el.getBoundingClientRect();
      // Nothing to see (and a 1 px screen-reader-only control is not meant to be tapped).
      if (r.width <= 2 || r.height <= 2) continue;
      // A field's label takes the tap too (a checkbox row): the larger of the two is the target.
      const label = (el as HTMLInputElement).labels?.[0];
      if (label) {
        const l = label.getBoundingClientRect();
        if (l.width * l.height > r.width * r.height) r = l;
      }
      const s = getComputedStyle(el);
      if (s.visibility === "hidden" || s.pointerEvents === "none" || (el as HTMLButtonElement).disabled) continue;
      // Below the fold still counts (it scrolls up); off to the side is a closed drawer or a strip's hidden part.
      if (r.right < 0 || r.left > innerWidth) continue;
      // A control inside a bigger control (an icon in a row) is reached through that one.
      const outer = el.parentElement?.closest(sel);
      if (outer) {
        const o = outer.getBoundingClientRect();
        if (o.width >= min && o.height >= min) continue;
      }
      if (r.width >= min && r.height >= min) continue;
      const text = (el.getAttribute("aria-label") || el.getAttribute("title") || label?.textContent || el.textContent
        || (el as HTMLInputElement).placeholder || el.getAttribute("type") || el.tagName)
        .trim().replace(/\s+/g, " ").slice(0, 40);
      out.push({ label: `${el.tagName.toLowerCase()} "${text}"`, width: Math.round(r.width), height: Math.round(r.height) });
    }
    return out;
  }, [min, within ?? ""] as [number, string]);
}

/**
 * An on-screen keyboard of `kb` px over the bottom of the screen. "resize": a phone browser shrinks the page to what
 * is above it. "inset": the apps keep the page's size and report the keyboard as `--kb` and `html.kb-open`
 * (upstream keyboardInset.ts), so the bottom `kb` px are covered.
 */
export async function openKeyboard(page: Page, kb: number, mode: "resize" | "inset"): Promise<{ visibleBottom: number }> {
  const { width, height } = page.viewportSize()!;
  if (mode === "resize") {
    await page.setViewportSize({ width, height: height - kb });
    return { visibleBottom: height - kb };
  }
  await page.evaluate((kb) => {
    const root = document.documentElement;
    root.style.setProperty("--kb", `${kb}px`);
    root.classList.add("kb-open");
  }, kb);
  return { visibleBottom: height - kb };
}

/** Wait until `locator` stops moving (phone sheets slide in), for up to 3 s. */
export async function settled(locator: ReturnType<Page["locator"]>): Promise<void> {
  await locator.waitFor({ state: "visible" });
  let last = "";
  for (let i = 0; i < 30; i++) {
    const box = JSON.stringify(await locator.boundingBox());
    if (box === last) return;
    last = box;
    await locator.page().waitForTimeout(100);
  }
}

/**
 * Scroll the way a finger can until `locator` is fully on screen above `visibleBottom`, and say whether it got there.
 * Only what a finger moves is scrolled: ancestors with `overflow-y: auto | scroll`, and the page itself unless the
 * root or body has `overflow: hidden` (upstream's base.css does, so the window never scrolls). A programmatic
 * `scrollIntoView` would also move `overflow: hidden` boxes, which a finger can't, so it would hide the bug.
 * (Playwright has no touch drag, and mobile WebKit no wheel, so this walks the scrollers itself.)
 */
export async function scrollIntoReach(page: Page, locator: ReturnType<Page["locator"]>, visibleBottom: number): Promise<boolean> {
  const { width } = page.viewportSize()!;
  const inReach = async () => {
    const box = await locator.boundingBox();
    return Boolean(box && box.y >= 0 && box.y + box.height <= visibleBottom + 0.5 && box.x >= -0.5 && box.x + box.width <= width + 0.5);
  };
  await settled(locator);
  if (await inReach()) return true;
  await locator.evaluate((el, bottom) => {
    const scrolls = (s: CSSStyleDeclaration) => s.overflowY === "auto" || s.overflowY === "scroll";
    const doc = document.scrollingElement as HTMLElement;
    const rootLocked = [document.documentElement, document.body].some((e) => {
      const s = getComputedStyle(e);
      return s.overflowY === "hidden" || s.overflow === "hidden" || s.overflowY === "clip";
    });
    const chain: HTMLElement[] = [];
    for (let p = el.parentElement; p; p = p.parentElement) {
      if (p === document.body || p === document.documentElement) continue;
      if (scrolls(getComputedStyle(p)) && p.scrollHeight > p.clientHeight) chain.push(p);
    }
    if (!rootLocked) chain.push(doc);
    // Innermost first, as a finger's drag goes to the innermost scroller that can still move.
    for (const s of chain) {
      const r = el.getBoundingClientRect();
      const view = s === doc ? { top: 0, bottom } : (() => {
        const b = s.getBoundingClientRect();
        return { top: Math.max(0, b.top), bottom: Math.min(bottom, b.bottom) };
      })();
      if (r.bottom > view.bottom) s.scrollTop += r.bottom - view.bottom + 4;
      else if (r.top < view.top) s.scrollTop -= view.top - r.top + 4;
    }
  }, visibleBottom);
  await page.waitForTimeout(50);
  return inReach();
}

export interface UnevenRow { container: string; boxes: string[]; heights: number[] }

/**
 * The user's rule (2026-11-14): "when boxes side by side always same height". Every row of boxes (a card or panel: a
 * child with a border or a background of its own, at least 48 px tall) laid out side by side by a grid or a row flex
 * container, on the page or inside `within`, whose heights differ by more than 2 px. Phones stack at natural height,
 * so this is for wide screens. A container can opt out with `data-natural-height` (a masonry list on purpose).
 */
export function unevenRows(page: Page, within?: string): Promise<UnevenRow[]> {
  return page.evaluate((within) => {
    const scope: ParentNode = (within && document.querySelector(within)) || document;
    const name = (el: Element) => `${el.tagName.toLowerCase()}${el.id ? `#${el.id}` : ""}${[...el.classList].slice(0, 3).map((c) => `.${c}`).join("")}${el.getAttribute("aria-label") ? `[${el.getAttribute("aria-label")}]` : ""}`;
    const boxy = (el: HTMLElement) => {
      const s = getComputedStyle(el);
      if (s.display === "none" || s.visibility === "hidden" || s.position === "absolute" || s.position === "fixed") return false;
      const border = ["Top", "Right", "Bottom", "Left"].some((k) => parseFloat(s[`border${k}Width` as "borderTopWidth"]) > 0 && s[`border${k}Style` as "borderTopStyle"] !== "none");
      const bg = s.backgroundColor !== "rgba(0, 0, 0, 0)" && s.backgroundColor !== "transparent";
      return border || bg;
    };
    const out: { container: string; boxes: string[]; heights: number[] }[] = [];
    const all = [scope instanceof Element ? scope : document.body, ...Array.from(scope.querySelectorAll<HTMLElement>("*"))];
    for (const c of all) {
      if (!(c instanceof HTMLElement) || c.closest("[data-natural-height]")) continue;
      const s = getComputedStyle(c);
      const grid = s.display.includes("grid");
      const flexRow = s.display.includes("flex") && s.flexDirection.startsWith("row");
      if (!grid && !flexRow) continue;
      const kids = Array.from(c.children).filter((k): k is HTMLElement => k instanceof HTMLElement && boxy(k));
      const rects = kids.map((k) => ({ k, r: k.getBoundingClientRect() })).filter(({ r }) => r.height >= 48 && r.width >= 120);
      const rows = new Map<number, typeof rects>();
      for (const x of rects) {
        const key = [...rows.keys()].find((t) => Math.abs(t - x.r.top) <= 3) ?? x.r.top;
        rows.set(key, [...(rows.get(key) ?? []), x]);
      }
      for (const row of rows.values()) {
        if (row.length < 2) continue;
        const hs = row.map(({ r }) => Math.round(r.height));
        if (Math.max(...hs) - Math.min(...hs) > 2) out.push({ container: name(c), boxes: row.map(({ k }) => name(k)), heights: hs });
      }
    }
    return out;
  }, within ?? "");
}
