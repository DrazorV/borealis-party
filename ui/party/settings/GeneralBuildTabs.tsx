import { BuildTab as UpstreamBuild, GeneralTab as UpstreamGeneral } from "../../src/pages/settings/GeneralBuildTabs";
import { Locked, field, formControls } from "./Locked";
import { rulesAccess } from "./scope";

export * from "../../src/pages/settings/GeneralBuildTabs";

type Props = Parameters<typeof UpstreamGeneral>[0];

/** GENERAL's one house rule. */
const method = field("Ability score generation option");

/** GENERAL: the account's own settings, plus the ability score method, which is a house rule. */
export function GeneralTab(props: Props) {
  const rules = rulesAccess();
  return (
    <Locked mode={rules === "edit" ? null : rules === "read" ? "read" : "hide"} pick={rules === "none" ? method.rows : method.controls}>
      <UpstreamGeneral {...props} />
    </Locked>
  );
}

/** CHARACTER BUILD: house rules only. */
export function BuildTab(props: Props) {
  const rules = rulesAccess();
  return (
    <Locked mode={rules === "read" ? "read" : null} pick={formControls} note>
      <UpstreamBuild {...props} />
    </Locked>
  );
}
