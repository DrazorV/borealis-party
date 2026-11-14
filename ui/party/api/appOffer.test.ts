import { describe, expect, it } from "vitest";
import { memoryStorage } from "../test/memoryStorage";
import { APP_ASSETS, addressToEnter, dismissOffer, isArm, offerDismissed, offerFor, releasesPage, visitorOs } from "./appOffer";

const PIXEL = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36";
const TAB = "Mozilla/5.0 (Linux; Android 13; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1";
const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";
const WINDOWS = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const LINUX = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const FIREFOX_LINUX = "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:132.0) Gecko/20100101 Firefox/132.0";
const CHROMEBOOK = "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";

const os = (userAgent: string, maxTouchPoints = 0, platform?: string) => visitorOs({ userAgent, maxTouchPoints, platform });
const BASE = "https://github.com/fortress-soft/borealis-party/releases/latest/download";

describe("visitorOs", () => {
  it("knows phones and tablets", () => {
    expect(os(PIXEL, 5)).toBe("android");
    expect(os(TAB, 5)).toBe("android");
    expect(os(IPHONE, 5)).toBe("ios");
    expect(os(MAC, 5)).toBe("ios");
  });
  it("knows computers", () => {
    expect(os(WINDOWS)).toBe("windows");
    expect(os(LINUX)).toBe("linux");
    expect(os(FIREFOX_LINUX)).toBe("linux");
    expect(os(MAC)).toBe("mac");
  });
  it("prefers userAgentData's platform where the browser offers it", () => {
    expect(os("Mozilla/5.0", 0, "Windows")).toBe("windows");
    expect(os("Mozilla/5.0", 0, "Linux")).toBe("linux");
    expect(os("Mozilla/5.0", 0, "macOS")).toBe("mac");
    expect(os("Mozilla/5.0", 5, "Android")).toBe("android");
  });
  it("leaves other systems alone", () => {
    expect(os(CHROMEBOOK)).toBeNull();
    expect(os("Mozilla/5.0", 0, "Chrome OS")).toBeNull();
    expect(os("curl/8.0")).toBeNull();
  });
});

describe("offerFor", () => {
  it("offers each system its app from the server's folder", () => {
    expect(offerFor(BASE, "android")).toEqual({ kind: "app", name: "Android", links: [{ label: "Download the app", href: `${BASE}/${APP_ASSETS.android}` }] });
    expect(offerFor("https://files.example.com/apps/", "windows")).toEqual({
      kind: "app", name: "Windows", links: [{ label: "Download the installer", href: `https://files.example.com/apps/${APP_ASSETS.windows}` }],
    });
  });
  it("offers Linux the AppImage first and the .deb second", () => {
    const offer = offerFor(BASE, "linux");
    expect(offer?.kind === "app" && offer.links.map((l) => l.href)).toEqual([`${BASE}/${APP_ASSETS.linuxAppImage}`, `${BASE}/${APP_ASSETS.linuxDeb}`]);
  });
  it("offers ARM Linux its own files only when the browser reports ARM", () => {
    const offer = offerFor(BASE, "linux", true);
    expect(offer?.kind === "app" && offer.links.map((l) => l.href)).toEqual([`${BASE}/${APP_ASSETS.linuxArmAppImage}`, `${BASE}/${APP_ASSETS.linuxArmDeb}`]);
    expect(isArm("Mozilla/5.0 (X11; Linux aarch64; rv:132.0) Gecko/20100101 Firefox/132.0")).toBe(true);
    expect(isArm(LINUX)).toBe(false);
    expect(isArm(LINUX, "arm")).toBe(true);
    expect(isArm("Mozilla/5.0 (X11; Linux aarch64)", "x86")).toBe(false);
  });
  it("offers iOS the unsigned IPA to sideload, and macOS a note", () => {
    expect(offerFor(BASE, "ios")).toEqual({
      kind: "app",
      name: "iPhone and iPad",
      links: [{ label: "Download the app", href: `${BASE}/${APP_ASSETS.iosUnsigned}` }],
      hint: expect.stringMatching(/AltStore, SideStore or Sideloadly.*7 days/),
    });
    expect(offerFor(BASE, "mac")).toMatchObject({ kind: "note", note: expect.stringMatching(/no macOS app/) });
  });
  it("offers nothing when the server turned downloads off", () => {
    expect(offerFor(null, "android")).toBeNull();
    expect(offerFor("", "linux")).toBeNull();
  });
});

describe("releasesPage", () => {
  it("is the GitHub release page, or the folder itself", () => {
    expect(releasesPage(BASE)).toBe("https://github.com/fortress-soft/borealis-party/releases/latest");
    expect(releasesPage("https://github.com/o/r/releases/download/party-v0.2.0/")).toBe("https://github.com/o/r/releases/tag/party-v0.2.0");
    expect(releasesPage("https://files.example.com/apps/")).toBe("https://files.example.com/apps");
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
