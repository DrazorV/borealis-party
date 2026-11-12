import { useEffect, useState } from "react";
import { IconButton } from "../../src/components/Button";
import { Icon } from "../../src/components/Icon";
import { isTauri } from "../../src/lib/tauri";
import { addressToEnter, appDownloadUrl, browserMobileOs, dismissOffer, offerDismissed, type MobileOs } from "../api/appOffer";
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

const deviceOs = (): MobileOs | null =>
  typeof navigator === "undefined" ? null : browserMobileOs(navigator.userAgent, navigator.maxTouchPoints ?? 0);

/**
 * A phone or tablet browser on the server's website: the app for this device, and the address to enter in it. On iOS,
 * which can't install an app from a website, a short note instead. Nothing inside the apps, on a computer, when the
 * server offers no download (`server.app_download = "off"`) or once closed in this browser.
 * `folder`: the download folder when the caller already has `/api/server`'s answer (the sign-in page).
 */
export function AppOffer({ folder }: { folder?: string | null } = {}) {
  const os = isTauri() ? null : deviceOs();
  const [closed, setClosed] = useState(offerDismissed);
  const [fetched, setFetched] = useState<string | null | undefined>(undefined);
  const wanted = Boolean(os) && !closed && folder === undefined;
  useEffect(() => {
    if (!wanted) return;
    let live = true;
    void downloadFolder().then((f) => { if (live) setFetched(f); });
    return () => { live = false; };
  }, [wanted]);
  const base = folder === undefined ? fetched : folder;
  if (!os || closed || !base) return null;

  const close = () => {
    dismissOffer();
    setClosed(true);
  };
  const dismiss = <IconButton icon="x" label="Dismiss" small onClick={close} />;
  const url = appDownloadUrl(base, os);
  if (!url) {
    return (
      <aside className="alert info party-app-offer" aria-label="About the app">
        <div className="party-app-offer-text">
          There is no iPhone or iPad app to install from a website: iOS only installs apps from the App Store or
          TestFlight. Borealis Party works here in Safari.
        </div>
        {dismiss}
      </aside>
    );
  }
  return (
    <aside className="alert info party-app-offer" aria-label="Get the app">
      <div className="party-app-offer-text">
        <strong>Get the Android app.</strong> It works offline and syncs with this server.
        <span>When it asks for the server address, enter <code className="party-app-address">{addressToEnter(window.location.origin)}</code></span>
        <a className="btn btn-primary btn-sm" href={url} download>
          <Icon name="download" />
          Download the app
        </a>
      </div>
      {dismiss}
    </aside>
  );
}
