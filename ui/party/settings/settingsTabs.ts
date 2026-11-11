import { visibleSettingsTabs as upstreamTabs, type SettingsTabDef, type TabGates } from "../../src/pages/settings/settingsTabs";

export * from "../../src/pages/settings/settingsTabs";

/** The server's administrator's (the AI assistant, pairing, updates, the license), or about the on-device search model: nothing an account can change. */
const SERVER_TABS: ReadonlySet<string> = new Set(["ai", "phones", "devices", "updates", "license", "search"]);

/** Upstream's tabs, less those a server has nothing behind. */
export function visibleSettingsTabs(gates: TabGates): SettingsTabDef[] {
  return upstreamTabs(gates).filter((t) => !SERVER_TABS.has(t.id));
}
