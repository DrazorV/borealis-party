import { afterEach, describe, expect, it } from "vitest";
import { openOverlayCount, pushOverlay } from "../../src/components/controls/overlayStack";
import { detectBrowserPhoneOs, installBrowserBack, OVERLAY_KEY, type BackWindow, type BrowserEnv } from "./phoneBrowser";

const ANDROID = "Mozilla/5.0 (Linux; Android 16; SM-S948B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36";
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
const IPAD_DESKTOP = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

const env = (o: Partial<BrowserEnv>): BrowserEnv => ({ userAgent: WINDOWS, maxTouchPoints: 0, search: "", isTauri: false, coarse: false, ...o });

describe("detectBrowserPhoneOs", () => {
  it("finds Android and iPhone browsers with a touch screen", () => {
    expect(detectBrowserPhoneOs(env({ userAgent: ANDROID, maxTouchPoints: 5, coarse: true }))).toBe("android");
    expect(detectBrowserPhoneOs(env({ userAgent: IPHONE, maxTouchPoints: 5, coarse: true }))).toBe("ios");
  });

  it("finds an iPad in desktop mode by its touch points", () => {
    expect(detectBrowserPhoneOs(env({ userAgent: IPAD_DESKTOP, maxTouchPoints: 5, coarse: true }))).toBe("ios");
    expect(detectBrowserPhoneOs(env({ userAgent: IPAD_DESKTOP }))).toBeNull();
  });

  it("leaves desktop browsers alone, even with a borrowed phone user agent", () => {
    expect(detectBrowserPhoneOs(env({}))).toBeNull();
    expect(detectBrowserPhoneOs(env({ maxTouchPoints: 10, coarse: true }))).toBeNull();
    expect(detectBrowserPhoneOs(env({ userAgent: ANDROID }))).toBeNull();
  });

  it("leaves the apps and the web mock to upstream's startPhone", () => {
    expect(detectBrowserPhoneOs(env({ userAgent: ANDROID, maxTouchPoints: 5, coarse: true, isTauri: true }))).toBeNull();
    expect(detectBrowserPhoneOs(env({ userAgent: IPHONE, maxTouchPoints: 5, coarse: true, search: "?mock=mobile" }))).toBeNull();
  });
});

/** A history with entries and popstate, close enough to a browser's for the Back handler. */
class FakeWindow implements BackWindow {
  entries: unknown[] = [{ idx: 0, key: "a" }];
  at = 0;
  private pop = new Set<() => void>();
  private timers: (() => void)[] = [];
  location = { href: "http://x/#/" };
  history = {
    get state() { return self.entries[self.at]; },
    pushState: (s: unknown) => { this.entries = [...this.entries.slice(0, this.at + 1), s]; this.at += 1; },
    go: (n?: number) => { this.travel(n ?? 0); },
    back: () => { this.travel(-1); },
    forward: () => { this.travel(1); },
  } as unknown as BackWindow["history"];
  constructor() { self = this; }
  addEventListener(_: "popstate", l: () => void) { this.pop.add(l); }
  removeEventListener(_: "popstate", l: () => void) { this.pop.delete(l); }
  setTimeout(h: () => void) { this.timers.push(h); }
  /** Runs pending timers (the deferred go). */
  flush() { while (this.timers.length) this.timers.shift()!(); }
  /** The router navigating: a new entry with the next index. */
  navigate(key: string) { this.history.pushState({ idx: ((this.history.state as { idx: number }).idx) + 1, key }, "", ""); }
  travel(n: number) {
    const next = Math.max(0, Math.min(this.entries.length - 1, this.at + n));
    if (next === this.at) return;
    this.at = next;
    this.pop.forEach((l) => l());
  }
}
let self: FakeWindow;

/** An overlay on the shared stack that unmounts when closed (the stack's entry goes when it unmounts). */
function overlay(closed: { n: number }): () => void {
  let remove: () => void = () => undefined;
  remove = pushOverlay(() => { closed.n += 1; remove(); });
  return () => remove();
}

describe("installBrowserBack", () => {
  const cleanups: (() => void)[] = [];
  afterEach(() => { cleanups.splice(0).forEach((c) => c()); expect(openOverlayCount()).toBe(0); });

  it("Back closes the open overlay and stays on the page", () => {
    const w = new FakeWindow();
    cleanups.push(installBrowserBack(w));
    const closed = { n: 0 };
    cleanups.push(overlay(closed));
    expect(w.entries).toHaveLength(2);
    expect((w.history.state as Record<string, unknown>)[OVERLAY_KEY]).toEqual(expect.any(Number));
    expect((w.history.state as Record<string, unknown>).key).toBe("a");
    w.history.back();
    expect(closed.n).toBe(1);
    expect(openOverlayCount()).toBe(0);
    expect(w.at).toBe(0);
    w.flush();
    expect(w.at).toBe(0);
  });

  it("an overlay closed by its own button takes its entry back off", () => {
    const w = new FakeWindow();
    w.navigate("b");
    cleanups.push(installBrowserBack(w));
    const close = overlay({ n: 0 });
    expect(w.at).toBe(2);
    close();
    w.flush();
    expect(w.at).toBe(1);
    // The next Back leaves the page as usual.
    w.history.back();
    expect(w.at).toBe(0);
  });

  it("closes nested overlays one Back at a time", () => {
    const w = new FakeWindow();
    cleanups.push(installBrowserBack(w));
    const outer = { n: 0 };
    const inner = { n: 0 };
    cleanups.push(overlay(outer), overlay(inner));
    expect(w.at).toBe(2);
    w.history.back();
    expect([outer.n, inner.n]).toEqual([0, 1]);
    w.history.back();
    expect([outer.n, inner.n]).toEqual([1, 1]);
    expect(w.at).toBe(0);
  });

  it("a jump back past several entries closes every overlay above it", () => {
    const w = new FakeWindow();
    cleanups.push(installBrowserBack(w));
    const a = { n: 0 };
    const b = { n: 0 };
    cleanups.push(overlay(a), overlay(b));
    w.history.go(-2);
    expect([a.n, b.n]).toEqual([1, 1]);
    expect(openOverlayCount()).toBe(0);
  });

  it("a sheet choice that navigates keeps the new page, and Back then skips the sheet's entry", () => {
    const w = new FakeWindow();
    cleanups.push(installBrowserBack(w));
    const close = overlay({ n: 0 });
    // A Menu item: the router pushes the new page, then the sheet unmounts.
    w.navigate("sources");
    close();
    w.flush();
    expect((w.history.state as Record<string, unknown>).key).toBe("sources");
    w.history.back();
    expect(w.at).toBe(0);
    // Forward goes on past it to the new page.
    w.history.forward();
    expect((w.history.state as Record<string, unknown>).key).toBe("sources");
  });

  it("an overlay that refuses to close keeps an entry for the next Back", () => {
    const w = new FakeWindow();
    cleanups.push(installBrowserBack(w));
    let remove: () => void = () => undefined;
    let asked = 0;
    remove = pushOverlay(() => { asked += 1; });
    cleanups.push(() => remove());
    w.history.back();
    expect(asked).toBe(1);
    expect(openOverlayCount()).toBe(1);
    // A change on the stack (here the prompt it opened, then closed) gives it its entry again.
    const prompt = pushOverlay(() => undefined);
    prompt();
    w.flush();
    w.history.back();
    expect(asked).toBe(2);
  });
});
