import { useNavigate } from "react-router";
import { useConfirm } from "../../components";
import { useCharacter } from "../../state/CharacterContext";
import { MissingContentDialog } from "../characters/MissingContentList";

/** GitHub #37: the notice's action that opens the list of what the import notice counts. */
export function ShowMissingContentButton({ onClick }: { onClick: () => void }) {
  return <>{" "}<button type="button" className="bp-linkbtn" onClick={onClick}>Show missing content</button></>;
}

/**
 * GitHub #37: the import notice counts the content the file names that is not installed; this lists it (ids, names, types, by
 * the source the ids name), with the way to add that content and re-check without re-importing. It stays open after a
 * re-check that finds everything, to say so.
 */
export function CharacterMissingContentDialog({ onClose }: { onClose: () => void }) {
  const ch = useCharacter();
  const nav = useNavigate();
  const confirm = useConfirm();
  const snapshot = ch.snapshot;
  if (!snapshot) return null;
  const recheck = () => void ch.mutate(async (api, characterId) => {
    await api.recheckMissingContent();
    return api.getSnapshot({ characterId });
  });
  // Instead of installing it: the choices of content that is not installed open again, to pick something else.
  const leaveUnselected = async () => {
    const r = await confirm({
      title: "Leave missing content unselected",
      body: `Remove the content that is not installed from ${snapshot.name}? Its choices are left open to pick something else, and options and items that are not installed are removed. Installing the content later does not bring them back.`,
      confirmLabel: "Leave unselected",
    });
    if (r !== "confirm") return;
    await ch.mutate((api, characterId) => api.clearMissingContent({ characterId }));
  };
  return (
    <MissingContentDialog entries={[{ name: snapshot.name, missing: snapshot.missingContent }]} rechecking={ch.busy}
      onClose={onClose} onOpenContent={() => { onClose(); nav("/content"); }} onRecheck={recheck}
      onLeaveUnselected={() => void leaveUnselected()} />
  );
}
