import { useEffect, useState } from "react";
import { IconButton } from "../../src/components/Button";
import { Icon } from "../../src/components/Icon";
import { isTauri } from "../../src/lib/tauri";
import { addressToEnter, dismissOffer, isArm, offerDismissed, offerFor, releasesPage, visitorOs, type VisitorOs } from "../api/appOffer";
import { checkServer } from "../api/party";

/** `/api/server`'s download folder, asked once per page load. */
let asked: Promise<string | null> | null = null;
function downloadFolder(): Promise<string | null> {
  asked ??= checkServer("").then((info) => info.appDownload ?? null, () => null);
  return asked;
}

/** Tests start each case with a fresh answer. */
export function forgetDownloadFolderForTests(): void {
  asked = null;
}

/** The part of `navigator.userAgentData` the offer reads (Chromium browsers only). */
interface UaData {
  platform?: string;
  getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string }>;
}
const uaData = (): UaData | undefined =>
  typeof navigator === "undefined" ? undefined : (navigator as Navigator & { userAgentData?: UaData }).userAgentData;

function browserOs(): VisitorOs | null {
  if (typeof navigator === "undefined") return null;
  return visitorOs({ userAgent: navigator.userAgent, maxTouchPoints: navigator.maxTouchPoints ?? 0, platform: uaData()?.platform });
}

/** Whether a Linux browser runs on ARM; the architecture comes from `userAgentData` where offered, after a moment. */
function useArm(linux: boolean): boolean {
  const [arm, setArm] = useState(() => linux && typeof navigator !== "undefined" && isArm(navigator.userAgent));
  useEffect(() => {
    const ask = uaData()?.getHighEntropyValues;
    if (!linux || !ask) return;
    let live = true;
    ask.call(uaData(), ["architecture"]).then(
      ({ architecture }) => { if (live && architecture) setArm(isArm(navigator.userAgent, architecture)); },
      () => { /* not told: the user agent's answer stays */ },
    );
    return () => { live = false; };
  }, [linux]);
  return arm;
}

/**
 * A browser on the server's website: the app for this system (Android, Windows, Linux) and the address to enter in it,
 * or a short note where there is none to install (iOS, macOS), with a link to every build. Nothing inside the apps, on
 * other systems, when the server offers no download (`server.app_download = "off"`) or once closed in this browser.
 * `folder`: the download folder when the caller already has `/api/server`'s answer (the sign-in page).
 */
export function AppOffer({ folder }: { folder?: string | null } = {}) {
  const os = isTauri() ? null : browserOs();
  const [closed, setClosed] = useState(offerDismissed);
  const arm = useArm(os === "linux");
  const [fetched, setFetched] = useState<string | null | undefined>(undefined);
  const wanted = Boolean(os) && !closed && folder === undefined;
  useEffect(() => {
    if (!wanted) return;
    let live = true;
    void downloadFolder().then((f) => { if (live) setFetched(f); });
    return () => { live = false; };
  }, [wanted]);
  const base = folder === undefined ? fetched : folder;
  const offer = os && base ? offerFor(base, os, arm) : null;
  if (!base || !offer || closed) return null;

  const close = () => {
    dismissOffer();
    setClosed(true);
  };
  const others = <a className="party-app-others" href={releasesPage(base)} target="_blank" rel="noreferrer">Other systems</a>;
  if (offer.kind === "note") {
    return (
      <aside className="alert info party-app-offer" aria-label="About the app">
        <div className="party-app-offer-text">
          <span>{offer.note}</span>
          {others}
        </div>
        <IconButton icon="x" label="Dismiss" small onClick={close} />
      </aside>
    );
  }
  const [main, ...more] = offer.links;
  return (
    <aside className="alert info party-app-offer" aria-label="Get the app">
      <div className="party-app-offer-text">
        <span><strong>Get the {offer.name} app.</strong> It works offline and syncs with this server.</span>
        <span>When it asks for the server address, enter <code className="party-app-address">{addressToEnter(window.location.origin)}</code></span>
        <span className="party-app-links">
          {main && (
            <a className="btn btn-primary btn-sm" href={main.href} download>
              <Icon name="download" />
              {main.label}
            </a>
          )}
          {more.map((l) => <a key={l.href} className="btn btn-sm" href={l.href} download>{l.label}</a>)}
          {others}
        </span>
      </div>
      <IconButton icon="x" label="Dismiss" small onClick={close} />
    </aside>
  );
}
