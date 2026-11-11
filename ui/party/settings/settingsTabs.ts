import { visibleSettingsTabs as upstreamTabs, type SettingsTabDef, type TabGates } from "../../src/pages/settings/settingsTabs";
import { isDm } from "../api/session";
import { rulesAccess } from "./scope";

export * from "../../src/pages/settings/settingsTabs";

/** The server's administrator's (folders, logs, the AI assistant, pairing, updates, the license): no client's to change. */
const SERVER_TABS: ReadonlySet<string> = new Set(["ai", "phones", "devices", "updates", "license"]);
/** A Dungeon Master's content library. */
const DM_TABS: ReadonlySet<string> = new Set(["data", "content"]);
/** Nothing but house rules, which a Library has none of. */
const RULE_TABS: ReadonlySet<string> = new Set(["build", "rules"]);

/** Upstream's tabs, less those this account can't change here. */
export function visibleSettingsTabs(gates: TabGates): SettingsTabDef[] {
  const dm = isDm();
  const rules = rulesAccess();
  return upstreamTabs(gates).filter((t) => !SERVER_TABS.has(t.id) && (dm || !DM_TABS.has(t.id)) && (rules !== "none" || !RULE_TABS.has(t.id)));
}
