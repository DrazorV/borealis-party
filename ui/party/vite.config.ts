import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig, type Plugin } from "vite";
import upstream from "../vite.config";

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

/**
 * The Borealis mark as the tab icon. Upstream's `index.html` has none, so the links are added here; the files are in
 * `public/` (`favicon.svg` is `.github/readme/favicon.svg`; the PNG and ICO fallbacks are rendered from it).
 */
function partyIcons(): Plugin {
  const link = (attrs: Record<string, string>) => ({ tag: "link", attrs, injectTo: "head" as const });
  return {
    name: "party-icons",
    transformIndexHtml: () => [
      link({ rel: "icon", href: "/favicon.ico", sizes: "48x48" }),
      link({ rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }),
      link({ rel: "icon", href: "/favicon-32.png", type: "image/png", sizes: "32x32" }),
      link({ rel: "apple-touch-icon", href: "/apple-touch-icon.png" }),
    ],
  };
}

export default mergeConfig(upstream, defineConfig({
  plugins: [partyEntry(), partyIcons()],
  publicDir: "party/public",
  build: { outDir: "party/dist", emptyOutDir: true },
}));
