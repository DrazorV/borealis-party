import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig, type Plugin } from "vite";
import upstream from "../vite.config";
import { cargoVersion } from "./version";

const local = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/**
 * The party build is upstream's app with some of its imports pointed at ours, so upstream's own files stay as they are:
 * the entry points in `src/main.tsx`, and the settings window's tabs, less what a server has nothing behind (`settings/`).
 * The build fails if upstream renames one of them, rather than silently shipping the plain app.
 */
const REPLACED: Readonly<Record<string, Readonly<Record<string, string>>>> = {
  "src/main.tsx": {
    "./api": local("./entry/api.ts"),
    "./App": local("./entry/App.tsx"),
    "./AppProviders": local("./entry/AppProviders.tsx"),
  },
  // A NavLink the shell marks "not current" stays so (entry/shellRouter.tsx): one selected START sub-tab.
  "src/components/shell/Chrome.tsx": { "react-router": local("./entry/shellRouter.tsx") },
  "src/components/shell/MobileChrome.tsx": { "react-router": local("./entry/shellRouter.tsx") },
  // Additional Content is the one import page: the Import section above upstream's packs (content/WithImport.tsx).
  "src/pages/content/ContentPage.tsx": { "./PacksTab": local("./content/WithImport.tsx") },
  // Installed packs by origin, with Remove all on each group (content/PackGroups.tsx).
  "src/pages/content/PacksTab.tsx": { "./PackGroups": local("./content/PackGroups.tsx") },
  "src/pages/settings/ApplicationSettings.tsx": {
    "./settingsTabs": local("./settings/settingsTabs.ts"),
    "./BorealisTabs": local("./settings/BorealisTabs.tsx"),
    "./DataFilesTab": local("./settings/DataFilesTab.tsx"),
  },
};

const wanted = Object.entries(REPLACED).flatMap(([file, sources]) => Object.keys(sources).map((source) => `${file}: ${source}`));

function partyEntry(): Plugin {
  const seen = new Set<string>();
  return {
    name: "party-entry",
    enforce: "pre",
    resolveId(source, importer) {
      const from = importer?.replace(/\\/g, "/");
      const file = from && Object.keys(REPLACED).find((f) => from.endsWith(`/ui/${f}`));
      const target = file && REPLACED[file]?.[source];
      if (!target) return null;
      seen.add(`${file}: ${source}`);
      return target;
    },
    buildEnd(error) {
      if (error) return;
      const missing = wanted.filter((s) => !seen.has(s));
      if (missing.length > 0) this.error(`upstream no longer imports: ${missing.join(", ")}`);
    },
  };
}

const TITLE = /<title>Borealis<\/title>/;

/**
 * Upstream's `index.html` with the party's tab title and the Borealis mark as the tab icon (upstream's page has none).
 * The icon files are in `public/` (`favicon.svg` is `.github/readme/favicon.svg`; the PNG and ICO fallbacks are
 * rendered from it). The build fails if upstream changes its title, rather than silently shipping the plain one.
 */
function partyPage(): Plugin {
  const link = (attrs: Record<string, string>) => ({ tag: "link", attrs, injectTo: "head" as const });
  return {
    name: "party-page",
    transformIndexHtml(html) {
      if (!TITLE.test(html)) throw new Error("party-page: upstream index.html no longer has <title>Borealis</title>");
      return {
        html: html.replace(TITLE, "<title>Borealis Party</title>"),
        tags: [
          link({ rel: "icon", href: "/favicon.ico", sizes: "48x48" }),
          link({ rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }),
          link({ rel: "icon", href: "/favicon-32.png", type: "image/png", sizes: "32x32" }),
          link({ rel: "apple-touch-icon", href: "/apple-touch-icon.png" }),
        ],
      };
    },
  };
}

/** The party version (./version.ts), shown where upstream shows its own. The Docker build copies party/Cargo.toml for it. */
export const PARTY_VERSION = cargoVersion(readFileSync(local("../../party/Cargo.toml"), "utf8"));

export default mergeConfig(upstream, defineConfig({
  plugins: [partyEntry(), partyPage()],
  define: { __APP_VERSION__: JSON.stringify(PARTY_VERSION) },
  publicDir: "party/public",
  build: { outDir: "party/dist", emptyOutDir: true },
}));
