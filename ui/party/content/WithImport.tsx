import { PacksTab as UpstreamPacksTab } from "../../src/pages/content/PacksTab";
import { ImportSection } from "../pages/ImportSection";
import "./content.css";

/**
 * Additional Content as the one import page (0.2.1): the Import section first, then upstream's own sections for what
 * is installed (packs, index files, 5etools addresses). Upstream's ContentPage imports this file in place of its
 * PacksTab (ui/party/vite.config.ts); upstream's own import controls on the page are hidden by ./content.css.
 */
export function PacksTab() {
  return (
    <>
      <ImportSection />
      <UpstreamPacksTab />
    </>
  );
}
