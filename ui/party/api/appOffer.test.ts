import { describe, expect, it } from "vitest";
import { memoryStorage } from "../test/memoryStorage";
import { APP_ASSETS, addressToEnter, appDownloadUrl, browserMobileOs, dismissOffer, offerDismissed } from "./appOffer";

const PIXEL = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36";
const TAB = "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
const IPAD = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";

describe("browserMobileOs", () => {
  it("knows Android phones and tablets, iPhones and iPads", () => {
    expect(browserMobileOs(PIXEL, 5)).toBe("android");
    expect(browserMobileOs(TAB, 5)).toBe("android");
    expect(browserMobileOs(IPHONE, 5)).toBe("ios");
    expect(browserMobileOs(IPAD, 5)).toBe("ios");
  });
  it("leaves computers alone, a Mac without touch included", () => {
    expect(browserMobileOs(WINDOWS, 0)).toBeNull();
    expect(browserMobileOs(IPAD, 0)).toBeNull();
  });
});

describe("appDownloadUrl", () => {
  const base = "https://github.com/DrazorV/borealis-party/releases/latest/download";
  it("puts the Android file in the server's folder", () => {
    expect(appDownloadUrl(base, "android")).toBe(`${base}/${APP_ASSETS.android}`);
    expect(appDownloadUrl("https://files.example.com/apps/", "android")).toBe(`https://files.example.com/apps/${APP_ASSETS.android}`);
  });
  it("offers nothing for iOS, nor when the server turned downloads off", () => {
    expect(appDownloadUrl(base, "ios")).toBeNull();
    expect(appDownloadUrl(null, "android")).toBeNull();
    expect(appDownloadUrl("", "android")).toBeNull();
  });
});

describe("addressToEnter", () => {
  it("is short when the app reads it back the same", () => {
    expect(addressToEnter("https://party.example.com")).toBe("party.example.com");
    expect(addressToEnter("http://192.168.1.20:8787")).toBe("192.168.1.20:8787");
    expect(addressToEnter("http://raspberrypi:8787")).toBe("raspberrypi:8787");
  });
  it("keeps the scheme the app would not guess", () => {
    expect(addressToEnter("http://party.example.com")).toBe("http://party.example.com");
    expect(addressToEnter("https://192.168.1.20:8443")).toBe("https://192.168.1.20:8443");
  });
});

describe("dismissal", () => {
  it("is remembered per browser", () => {
    const storage = memoryStorage();
    expect(offerDismissed(() => storage)).toBe(false);
    dismissOffer(() => storage);
    expect(offerDismissed(() => storage)).toBe(true);
  });
  it("survives storage that throws", () => {
    const broken = () => { throw new Error("blocked"); };
    expect(() => dismissOffer(broken)).not.toThrow();
    expect(offerDismissed(broken)).toBe(false);
  });
});
