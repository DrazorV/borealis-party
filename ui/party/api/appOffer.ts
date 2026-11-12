import { normaliseAddress } from "./session";

/**
 * The release files the apps come in, inside the server's download folder (`/api/server` `appDownload`, by default the
 * fork's latest GitHub release). Keep in step with the release workflow (party/README.md, "Download the apps"); phones
 * get the arm64 APK.
 */
export const APP_ASSETS = {
  android: "borealis-party-arm64.apk",
  ios: "borealis-party-ios.ipa",
  windows: "borealis-party-windows-x64-setup.exe",
  linuxAppImage: "borealis-party-linux-x86_64.AppImage",
  linuxDeb: "borealis-party-linux-amd64.deb",
  linuxArmAppImage: "borealis-party-linux-aarch64.AppImage",
  linuxArmDeb: "borealis-party-linux-arm64.deb",
} as const;

/**
 * Whether a browser can install the iOS file. Not today: the release's iOS file is an ad hoc build, which installs
 * only on the devices listed in its profile, so a website can't offer it to everyone and the page says so instead.
 */
export const IOS_INSTALLS_FROM_WEB = false;

/** The systems the offer tells apart. */
export type VisitorOs = "android" | "ios" | "windows" | "linux" | "mac";

/** What the browser says about itself: its user agent, touch points and, where offered, `navigator.userAgentData.platform`. */
export interface BrowserEnv {
  userAgent: string;
  maxTouchPoints: number;
  platform?: string;
}

/**
 * The visitor's system. Android before Linux (its user agent names both); an iPad asks for the desktop site and reports
 * a Mac with touch. Null for anything else, a Chromebook included.
 */
export function visitorOs({ userAgent, maxTouchPoints, platform = "" }: BrowserEnv): VisitorOs | null {
  const p = platform.toLowerCase();
  if (p === "android" || /Android/i.test(userAgent)) return "android";
  if (p === "ios" || /iPhone|iPad|iPod/i.test(userAgent) || (maxTouchPoints > 1 && /Macintosh/.test(userAgent))) return "ios";
  if (p === "chrome os" || p === "chromeos" || /CrOS/.test(userAgent)) return null;
  if (p === "windows" || /Windows/.test(userAgent)) return "windows";
  if (p === "macos" || /Macintosh|Mac OS X/.test(userAgent)) return "mac";
  if (p === "linux" || /Linux|X11/.test(userAgent)) return "linux";
  return null;
}

export interface OfferLink {
  label: string;
  href: string;
}

/** What the page offers a system: the app's downloads (the first is the main one), or a note when there is none. */
export type Offer =
  | { kind: "app"; name: string; links: OfferLink[] }
  | { kind: "note"; note: string };

const IOS_NOTE = "There is no iPhone or iPad app to install from here: iOS installs apps from the App Store, or ad hoc "
  + "builds only on devices registered in advance. Borealis Party works here in Safari.";
const MAC_NOTE = "There is no macOS app yet. Borealis Party works here in your browser.";

/**
 * Whether the browser runs on ARM: `userAgentData`'s architecture where the browser gives it (Chrome's user agent
 * always says x86_64 on Linux), else the user agent (Firefox says `aarch64`).
 */
export function isArm(userAgent: string, architecture?: string): boolean {
  if (architecture) return architecture.toLowerCase() === "arm";
  return /aarch64|arm64|armv\d/i.test(userAgent);
}

/** The offer for `os` from the folder `base`; null when the server offers no downloads. `arm`: Linux on ARM. */
export function offerFor(base: string | null | undefined, os: VisitorOs, arm = false): Offer | null {
  if (!base) return null;
  const at = (asset: string) => `${base.replace(/\/+$/, "")}/${asset}`;
  switch (os) {
    case "android":
      return { kind: "app", name: "Android", links: [{ label: "Download the app", href: at(APP_ASSETS.android) }] };
    case "windows":
      return { kind: "app", name: "Windows", links: [{ label: "Download the installer", href: at(APP_ASSETS.windows) }] };
    case "linux":
      return {
        kind: "app",
        name: "Linux",
        links: [
          { label: "Download the AppImage", href: at(arm ? APP_ASSETS.linuxArmAppImage : APP_ASSETS.linuxAppImage) },
          { label: ".deb package", href: at(arm ? APP_ASSETS.linuxArmDeb : APP_ASSETS.linuxDeb) },
        ],
      };
    case "ios":
      return IOS_INSTALLS_FROM_WEB
        ? { kind: "app", name: "iOS", links: [{ label: "Download the app", href: at(APP_ASSETS.ios) }] }
        : { kind: "note", note: IOS_NOTE };
    case "mac":
      return { kind: "note", note: MAC_NOTE };
  }
}

/** Where every build is listed: the release page for a GitHub release's download folder, else the folder itself. */
export function releasesPage(base: string): string {
  const trimmed = base.replace(/\/+$/, "");
  if (/\/releases\/latest\/download$/.test(trimmed)) return trimmed.replace(/\/download$/, "");
  return trimmed.replace(/\/releases\/download\/([^/]+)$/, "/releases/tag/$1");
}

/**
 * What to type into the app's server address box to reach the server this page came from: the short form when the
 * app reads it back as the same address (a domain over https, an IP address over http), else the full origin.
 */
export function addressToEnter(origin: string): string {
  const short = origin.replace(/^https?:\/\//i, "");
  return normaliseAddress(short) === origin.toLowerCase() ? short : origin;
}

const DISMISSED = "borealis-party.app-offer-dismissed";

/** Whether this browser closed the offer before. Unreadable storage counts as not closed. */
export function offerDismissed(storage: () => Storage = () => localStorage): boolean {
  try {
    return storage().getItem(DISMISSED) === "1";
  } catch {
    return false;
  }
}

/** Remember in this browser that the offer was closed; without storage it stays closed until the page reloads. */
export function dismissOffer(storage: () => Storage = () => localStorage): void {
  try {
    storage().setItem(DISMISSED, "1");
  } catch { /* private mode or blocked storage */ }
}
