import { normaliseAddress } from "./session";

/**
 * The release files the apps come in, inside the server's download folder (`/api/server` `appDownload`, by default the
 * fork's latest GitHub release). Keep in step with the release workflow (party/README.md, "Download the apps"); phones get the arm64 APK.
 */
export const APP_ASSETS = {
  android: "borealis-party-arm64.apk",
  ios: "borealis-party-ios.ipa",
} as const;

/**
 * Whether a phone browser can install the iOS file. Not today: the release's iOS file is an ad hoc build, which installs
 * only on the devices listed in its profile, so a website can't offer it to everyone and the page says so instead.
 */
export const IOS_INSTALLS_FROM_WEB = false;

export type MobileOs = keyof typeof APP_ASSETS;

/** The phone or tablet a browser runs on, from its user agent (iPadOS reports a Mac with touch). Null on a computer. */
export function browserMobileOs(userAgent: string, maxTouchPoints: number): MobileOs | null {
  if (/Android/i.test(userAgent)) return "android";
  if (/iPhone|iPad|iPod/i.test(userAgent) || (maxTouchPoints > 1 && /Macintosh/.test(userAgent))) return "ios";
  return null;
}

/** The download link for `os` in the folder `base`, or null when there is none to offer. */
export function appDownloadUrl(base: string | null | undefined, os: MobileOs): string | null {
  if (!base) return null;
  if (os === "ios" && !IOS_INSTALLS_FROM_WEB) return null;
  return `${base.replace(/\/+$/, "")}/${APP_ASSETS[os]}`;
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
