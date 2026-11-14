import { useState } from "react";
import { BuildTab as UpstreamBuild, GeneralTab as UpstreamGeneral } from "../../src/pages/settings/GeneralBuildTabs";
import { DescCheck } from "../../src/pages/settings/rows";
import { useApi } from "../../src/state/ApiContext";
import { useCharacter } from "../../src/state/CharacterContext";
import { useSettings } from "../../src/state/SettingsContext";
import { setStatus } from "../../src/state/status";
import { errorMessage } from "../../src/state/useAsync";
import { personalitySkipped, SKIP_KEY } from "../api/personality";
import { setDigitalDiceHidden, useDigitalDiceHidden } from "../play/dicePrefs";

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

export const HIDE_DICE_TITLE = "Hide Roll for me";
export const HIDE_DICE_DESCRIPTION =
  "Every roll on the Play tab asks only for the total of your real dice: the app's own dice are hidden. Kept on this device.";

/** "Hide Roll for me" (../play/dicePrefs.ts): a device's own switch, saved at once. */
export function HideDiceCheck() {
  const hidden = useDigitalDiceHidden();
  return <DescCheck spaced title={HIDE_DICE_TITLE} description={HIDE_DICE_DESCRIPTION} checked={hidden} onChange={setDigitalDiceHidden} />;
}

/** GENERAL: upstream's, and the device's "Hide Roll for me" (0.3.5, typed table rolls first). */
export function GeneralTab(props: Parameters<typeof UpstreamGeneral>[0]) {
  return (
    <>
      <UpstreamGeneral {...props} />
      <HideDiceCheck />
    </>
  );
}
