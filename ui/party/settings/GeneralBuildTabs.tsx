import { useState } from "react";
import { BuildTab as UpstreamBuild } from "../../src/pages/settings/GeneralBuildTabs";
import { DescCheck } from "../../src/pages/settings/rows";
import { useApi } from "../../src/state/ApiContext";
import { useCharacter } from "../../src/state/CharacterContext";
import { useSettings } from "../../src/state/SettingsContext";
import { setStatus } from "../../src/state/status";
import { errorMessage } from "../../src/state/useAsync";
import { personalitySkipped, SKIP_KEY } from "../api/personality";

export * from "../../src/pages/settings/GeneralBuildTabs";

export const SKIP_TITLE = "Skip Personality Choices";
export const SKIP_DESCRIPTION =
  "Next selection and the open-choice counts leave out personality traits, ideals, bonds and flaws. You can still pick them in the Background section.";

/**
 * The account's "Skip Personality Choices" (../api/overrides/personality.ts). Upstream's form keeps only its own fields
 * until Save, so this one is saved at once, like the Search tab's switches, and the open character is read again so its
 * Next selection and badges follow.
 */
export function SkipPersonalityCheck() {
  const api = useApi();
  const { replace } = useSettings();
  const { snapshot, mutate } = useCharacter();
  const [checked, setChecked] = useState(personalitySkipped);
  const [busy, setBusy] = useState(false);
  const change = async (v: boolean) => {
    setBusy(true);
    setChecked(v);
    try {
      replace(await api.saveSettings({ patch: { [SKIP_KEY]: v } as never }));
      setChecked(personalitySkipped());
      if (snapshot) await mutate((a, id) => a.getSnapshot({ characterId: id }));
    } catch (e) {
      setChecked(personalitySkipped());
      setStatus(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return <DescCheck spaced title={SKIP_TITLE} description={SKIP_DESCRIPTION} checked={checked} disabled={busy} onChange={(v) => void change(v)} />;
}

/** BUILD: upstream's, and the account's personality-choice switch. */
export function BuildTab(props: Parameters<typeof UpstreamBuild>[0]) {
  return (
    <>
      <UpstreamBuild {...props} />
      <SkipPersonalityCheck />
    </>
  );
}
