import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { normaliseAddress } from "../api/session";

// The app settings that live outside upstream's projects (party/android, party/ios, party/tauri.party.*.json) must
// give the Android, iOS and desktop apps the same identity and the same network policy.
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const read = (path: string): string => readFileSync(join(root, path), "utf8");

/** The value after `<key>name</key>` in a plist, comments removed. */
function plistValue(plist: string, key: string): string | null {
  const body = plist.replace(/<!--[\s\S]*?-->/g, "");
  const m = new RegExp(`<key>${key}</key>\\s*(<(\\w+)\\s*/>|<(\\w+)>([\\s\\S]*?)</\\3>)`).exec(body);
  if (!m) return null;
  return m[2] ?? m[4]?.trim() ?? null;
}

describe("the party apps' identity", () => {
  const ios = JSON.parse(read("party/tauri.party.ios.json")) as { identifier: string; bundle: { iOS: { infoPlist: string } } };
  const desktop = JSON.parse(read("party/tauri.party.desktop.json")) as { identifier: string; productName: string };
  const gradle = read("party/android/party.init.gradle");
  const strings = read("party/android/res/values/strings.xml");
  const plist = read("party/ios/Info.plist");

  it("one app id on Android, iOS and the desktop", () => {
    expect(gradle).toContain(`applicationId.set("${desktop.identifier}")`);
    expect(ios.identifier).toBe(desktop.identifier);
  });

  it("one name on Android, iOS and the desktop", () => {
    expect(strings).toContain(`<string name="app_name">"${desktop.productName}"</string>`);
    expect(plistValue(plist, "CFBundleDisplayName")).toBe(desktop.productName);
  });

  it("iOS merges party/ios/Info.plist, a path Tauri reads from src-tauri", () => {
    expect(existsSync(join(root, "src-tauri", ios.bundle.iOS.infoPlist))).toBe(true);
    expect(resolve(root, "src-tauri", ios.bundle.iOS.infoPlist)).toBe(join(root, "party", "ios", "Info.plist"));
  });

  it("the iOS app id is not in the shared tauri.party.json, which the Android build reads too", () => {
    expect(JSON.parse(read("party/tauri.party.json"))).not.toHaveProperty("identifier");
  });
});

describe("plain HTTP for home-network servers", () => {
  const plist = read("party/ios/Info.plist");

  it("the addresses that mean http:// on every app", () => {
    expect(normaliseAddress("192.168.1.20:8787")).toBe("http://192.168.1.20:8787");
    expect(normaliseAddress("raspberrypi.local:8787")).toBe("http://raspberrypi.local:8787");
    expect(normaliseAddress("party.example.com")).toBe("https://party.example.com");
  });

  it("Android allows cleartext traffic", () => {
    expect(read("party/android/party.init.gradle")).toContain(`put("usesCleartextTraffic", "true")`);
  });

  it("iOS: App Transport Security allows it, with no key that would make iOS ignore that", () => {
    const ats = /<key>NSAppTransportSecurity<\/key>\s*<dict>([\s\S]*?)<\/dict>/.exec(plist.replace(/<!--[\s\S]*?-->/g, ""))?.[1] ?? "";
    expect(plistValue(ats, "NSAllowsArbitraryLoads")).toBe("true");
    for (const key of ["NSAllowsLocalNetworking", "NSAllowsArbitraryLoadsInWebContent", "NSAllowsArbitraryLoadsForMedia"]) {
      expect(ats).not.toContain(key);
    }
  });

  it("iOS: the local network prompt names the party server, not upstream's device transfer", () => {
    expect(plistValue(plist, "NSLocalNetworkUsageDescription")).toMatch(/party server/);
  });

  it("the web view may reach any http or https server (CSP)", () => {
    const csp = (JSON.parse(read("party/tauri.party.json")) as { app: { security: { csp: string } } }).app.security.csp;
    expect(csp).toMatch(/connect-src [^;]*\bhttps: http:/);
  });
});
