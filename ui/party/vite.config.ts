import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig, type Plugin } from "vite";
import upstream from "../vite.config";

const local = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/**
 * The party build is upstream's app with three imports of `src/main.tsx` pointed at ours, so upstream's own files stay
 * as they are. The build fails if upstream renames one of them, rather than silently shipping the plain app.
 */
const REPLACED: Readonly<Record<string, string>> = {
  "./api": local("./entry/api.ts"),
  "./App": local("./entry/App.tsx"),
  "./AppProviders": local("./entry/AppProviders.tsx"),
};

function partyEntry(): Plugin {
  const seen = new Set<string>();
  return {
    name: "party-entry",
    enforce: "pre",
    resolveId(source, importer) {
      if (!importer?.replace(/\\/g, "/").endsWith("/src/main.tsx")) return null;
      const target = REPLACED[source];
      if (!target) return null;
      seen.add(source);
      return target;
    },
    buildEnd(error) {
      if (error) return;
      const missing = Object.keys(REPLACED).filter((s) => !seen.has(s));
      if (missing.length > 0) this.error(`upstream's src/main.tsx no longer imports: ${missing.join(", ")}`);
    },
  };
}

export default mergeConfig(upstream, defineConfig({
  plugins: [partyEntry()],
  build: { outDir: "party/dist", emptyOutDir: true },
}));
