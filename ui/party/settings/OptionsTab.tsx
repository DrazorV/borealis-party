import { HouseRules as UpstreamHouseRules } from "../../src/pages/manage/OptionsTab";
import { Locked, allControls } from "./Locked";
import { rulesAccess } from "./scope";

export * from "../../src/pages/manage/OptionsTab";

/**
 * A character's own house rules (Edition & compatibility). In a group they are the Dungeon Master's, so a member sees
 * them locked; the server keeps a member's character at the defaults whatever the client sends.
 */
export function HouseRules() {
  return (
    <Locked mode={rulesAccess() === "read" ? "read" : null} pick={allControls} note>
      <UpstreamHouseRules />
    </Locked>
  );
}
