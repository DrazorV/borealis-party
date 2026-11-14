import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { applyBrand, BRAND_EDITS, brandedFile, INSTALLED_NAME, PRODUCT_NAME } from "./branding";

/** Vitest runs in ui/. */
const upstream = (f: string) => readFileSync(join(process.cwd(), f), "utf8");

describe("party branding (user: \"make it say borealis party not borealis\")", () => {
  it("every edit finds its text exactly once in upstream's file as it is today, and leaves no plain name behind", () => {
    for (const [file, edits] of Object.entries(BRAND_EDITS)) {
      const out = applyBrand(file, upstream(file));
      for (const e of edits) {
        expect(out).toContain(e.to);
        expect(out).not.toContain(e.from);
      }
    }
    expect(applyBrand("src/components/shell/AppShell.tsx", upstream("src/components/shell/AppShell.tsx")))
      .toContain(`<div className="aur-title-text">${PRODUCT_NAME}</div>`);
  });

  it("fails loudly when upstream changes the text, instead of shipping the plain name", () => {
    expect(() => applyBrand("src/components/shell/AppShell.tsx", "<div>Something else</div>")).toThrow(/no longer has exactly one/);
  });

  it("matches the files by path from any checkout, Windows or not, with a query", () => {
    expect(brandedFile("C:\\x\\ui\\src\\components\\shell\\AppShell.tsx")).toBe("src/components/shell/AppShell.tsx");
    expect(brandedFile("/w/ui/src/components/splash/SplashView.tsx?v=1")).toBe("src/components/splash/SplashView.tsx");
    expect(brandedFile("/w/ui/src/components/shell/Chrome.tsx")).toBeNull();
  });

  it("the apps' own names are already the party's", () => {
    const repo = (f: string) => readFileSync(join(process.cwd(), "..", f), "utf8");
    // productName is the installed name (install folder, uninstall key), kept so updates replace the installed app.
    const desktop = JSON.parse(repo("party/tauri.party.desktop.json"));
    expect(desktop).toMatchObject({ productName: INSTALLED_NAME, mainBinaryName: "borealis-party", identifier: "app.borealis.party" });
    expect(desktop.app.windows[0].title).toBe(PRODUCT_NAME);
    expect(repo("party/android/res/values/strings.xml")).toContain(`<string name="main_activity_title">"${PRODUCT_NAME}"</string>`);
    expect(repo("party/android/res/values/strings.xml")).toContain(`<string name="app_name">"${PRODUCT_NAME}"</string>`);
    expect(repo("party/ios/Info.plist")).toMatch(new RegExp(`<key>CFBundleDisplayName</key>\\s*<string>${PRODUCT_NAME}</string>`));
  });
});
