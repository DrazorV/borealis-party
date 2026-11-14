import { defineConfig, devices } from "@playwright/test";

/**
 * Phone UI tests (plan M7a). Three phones, all through upstream's web mock `?mock=mobile`, which turns the phone layer
 * on in a browser:
 *  - android-portrait / android-landscape: Chromium with an Android Chrome user agent at the S26 Ultra's CSS size;
 *  - iphone: WebKit as iOS Safari at 390x844;
 *  - desktop: Chromium at 1280x800 without the phone layer, for unify-imports.spec.ts, combat-tracker.spec.ts,
 *    content-refresh.spec.ts and content-compact.spec.ts (which sets 1440x900);
 *  - desktop-wide: Chromium at 1440x900, a DM's big screen, for encounter-tab.spec.ts, spell-combat.spec.ts, combat-end.spec.ts, ai-guides.spec.ts and source-restrict.spec.ts.
 * The server is a throwaway local one (server/start.ts).
 */
const ANDROID_UA =
  "Mozilla/5.0 (Linux; Android 16; SM-S948B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36";
const android = {
  browserName: "chromium" as const,
  userAgent: ANDROID_UA,
  deviceScaleFactor: 2.8125, // 1080 px wide at 384 CSS px
  isMobile: true,
  hasTouch: true,
};

export default defineConfig({
  testDir: "tests",
  globalSetup: "./server/start.ts",
  // One server for the run; each test signs in with its own session.
  fullyParallel: true,
  workers: process.env.CI ? 2 : 4,
  retries: process.env.CI ? 1 : 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  forbidOnly: Boolean(process.env.CI),
  reporter: [
    ["list"],
    ["html", { open: "never" }],
    ["./findings-reporter.ts"],
  ],
  use: {
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "android-portrait", use: { ...android, viewport: { width: 384, height: 832 } } },
    { name: "android-landscape", use: { ...android, viewport: { width: 832, height: 384 } } },
    { name: "iphone", use: { ...devices["iPhone 14"], viewport: { width: 390, height: 844 } } },
    // A desktop browser window, for the specs that cover phones and desktops alike.
    { name: "desktop", testMatch: /(unify-imports|combat-tracker|content-refresh|content-compact|branding)\.spec\.ts/, use: { ...devices["Desktop Chrome"], viewport: { width: 1280, height: 800 } } },
    // A DM's big screen for session prep (0.3.1): the Encounters and Combat tabs use the whole width.
    { name: "desktop-wide", testMatch: /(encounter-tab|spell-combat|combat-end|ai-guides|source-restrict)\.spec\.ts/, use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } },
  ],
});
