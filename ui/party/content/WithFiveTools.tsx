import { FiveToolsTab as UpstreamFiveToolsTab } from "../../src/pages/content/FiveToolsTab";
import { ContentActivity } from "./ContentActivity";
import { ContentScope } from "./ContentScope";

/**
 * Additional Content's 5etools section, its list of addresses following the account's content changes at once
 * (./ContentScope.tsx), with what is on its way to the server above it (./ContentActivity.tsx). Upstream's
 * ContentPage imports this file in place of its FiveToolsTab (ui/party/vite.config.ts).
 */
const FIVETOOLS = ["fivetools"] as const;

export function FiveToolsTab() {
  return (
    <>
      <ContentActivity areas={FIVETOOLS} />
      <ContentScope><UpstreamFiveToolsTab /></ContentScope>
    </>
  );
}
