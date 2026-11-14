/**
 * The product name a user reads in the party build is "Borealis Connect" (the user, 2026-11-14; before that
 * "Borealis Party"). Upstream writes "Borealis" as plain text in a few of its own files; the
 * party build rewrites exactly those strings when it compiles them (vite.config.ts `partyBrand`), so upstream's files
 * stay as they are. Each edit must match once: the build fails if upstream changes the text, rather than shipping the
 * plain name.
 *
 * Elsewhere the name is already the party's: the page title (vite.config.ts `partyPage`), the desktop app's window
 * title (party/tauri.party.desktop.json), the Android label (party/android/res/values/strings.xml) and the iPhone's
 * (party/ios/Info.plist CFBundleDisplayName). The desktop app's Tauri productName stays {@link INSTALLED_NAME}: the
 * Windows installer derives its install folder and uninstall key from it, so a new name would install a second copy.
 */
export const PRODUCT_NAME = "Borealis Connect";

/** The desktop app's Tauri productName, kept from before the rename so installed apps update in place. */
export const INSTALLED_NAME = "Borealis Party";

export interface BrandEdit { from: string; to: string }

export const BRAND_EDITS: Readonly<Record<string, readonly BrandEdit[]>> = {
  // The desktop title bar, centered above the main tabs (shown upper case by its CSS).
  "src/components/shell/AppShell.tsx": [
    { from: `<div className="aur-title-text">Borealis</div>`, to: `<div className="aur-title-text">${PRODUCT_NAME}</div>` },
  ],
  // A phone's title bar with no character open.
  "src/components/shell/MobileChrome.tsx": [{ from: `snapshot ? snapshot.name : "Borealis"`, to: `snapshot ? snapshot.name : "${PRODUCT_NAME}"` }],
  // The desktop app's start-up splash, and its error title.
  "src/components/splash/SplashView.tsx": [
    { from: `<div className="splash-wordmark">Borealis</div>`, to: `<div className="splash-wordmark">${PRODUCT_NAME}</div>` },
    { from: `<h2 className="splash-error-title">Borealis could not start</h2>`, to: `<h2 className="splash-error-title">${PRODUCT_NAME} could not start</h2>` },
  ],
  // The Start page's heading.
  "src/pages/characters/CharactersPage.tsx": [
    { from: `<h1 className="v4-section">Welcome to Borealis</h1>`, to: `<h1 className="v4-section">Welcome to ${PRODUCT_NAME}</h1>` },
  ],
  // The name of the shared log file (Settings, Logs).
  "src/pages/settings/LogsRow.tsx": [{ from: `const SHARE_TITLE = "Borealis logs";`, to: `const SHARE_TITLE = "${PRODUCT_NAME} logs";` }],
};

/** The upstream file `id` names, as a key of {@link BRAND_EDITS}, or null. */
export function brandedFile(id: string): string | null {
  const path = id.replace(/\\/g, "/").split("?")[0]!;
  return Object.keys(BRAND_EDITS).find((f) => path.endsWith(`/ui/${f}`)) ?? null;
}

/** The file's code with its edits made; throws when an edit's text isn't there exactly once. */
export function applyBrand(file: string, code: string): string {
  let out = code;
  for (const e of BRAND_EDITS[file] ?? []) {
    const at = out.indexOf(e.from);
    if (at < 0 || out.indexOf(e.from, at + 1) >= 0) {
      throw new Error(`party-brand: upstream ${file} no longer has exactly one ${JSON.stringify(e.from)}`);
    }
    out = out.slice(0, at) + e.to + out.slice(at + e.from.length);
  }
  return out;
}
