import { PacksTab as UpstreamPacksTab } from "../../src/pages/content/PacksTab";
import { ImportSection } from "../pages/ImportSection";
import { BestiaryCoverage } from "./BestiaryCoverage";
import { ContentActivity } from "./ContentActivity";
import { ContentScope } from "./ContentScope";
import { UpdateAllCard } from "./UpdateAllCard";
import "./content.css";

/**
 * Additional Content as the one import page (0.2.1): the Import section first, then upstream's own sections for what
 * is installed (packs, index files, 5etools addresses), then the Bestiary report of what those sources hold in
 * creatures (0.3.0). Upstream's ContentPage imports this file in place of its PacksTab (ui/party/vite.config.ts);
 * upstream's own import controls on the page are hidden by ./content.css.
 * The packs list follows the account's content changes at once (./ContentScope.tsx), with what is on its way to the
 * server above it (./ContentActivity.tsx). Since 0.3.3 one "Update all content" (./UpdateAllCard.tsx) updates the index
 * files and every 5etools source; upstream's own "Update content files" and its list of 5etools addresses are hidden
 * (./content.css), so each source shows once, in the Import section's 5etools card.
 */
const PACKS = ["packs"] as const;

export function PacksTab() {
  return (
    <>
      <ImportSection />
      <ContentActivity areas={PACKS} />
      <UpdateAllCard />
      <ContentScope><UpstreamPacksTab /></ContentScope>
      <BestiaryCoverage />
    </>
  );
}
