import { ContentTab as UpstreamContent, RulesTab as UpstreamRules } from "../../src/pages/settings/BorealisTabs";
import { Locked, formControls } from "./Locked";
import { rulesAccess } from "./scope";

export * from "../../src/pages/settings/BorealisTabs";

/** RULES: house rules. A character's own house rules stay reachable from here, so only the fields are locked. */
export function RulesTab(props: Parameters<typeof UpstreamRules>[0]) {
  return (
    <Locked mode={rulesAccess() === "read" ? "read" : null} pick={formControls} note>
      <UpstreamRules {...props} />
    </Locked>
  );
}

/** CONTENT: the DM's packs and indexes. The portrait library and the notes vault are folders on the server. */
export function ContentTab(props: Parameters<typeof UpstreamContent>[0]) {
  return <UpstreamContent {...props} portraits={null} vault={null} />;
}
