import { useEffect, useRef, useState } from "react";
import { MessageBox } from "../../src/components";
import { useCharacter } from "../../src/state/CharacterContext";
import { serverFetch } from "../api/http";
import { loadSession } from "../api/session";
import { settingsPlace } from "../settings/scope";

/**
 * When a character comes into a group, or the DM changes the group's rules, the server takes out what those rules don't
 * allow (`apply_group_rules` in `party/crates/borealis-server/src/backend.rs`) and keeps a note of it. This tells the
 * player what to redo, once, the next time they open the character.
 */
export function GroupRulesNotice() {
  const { snapshot } = useCharacter();
  const id = snapshot?.id;
  // Read when the notes arrive: a rename is no reason to ask again.
  const name = useRef("");
  name.current = snapshot?.name ?? "";
  const [notes, setNotes] = useState<{ name: string; lines: string[] } | null>(null);
  useEffect(() => {
    const session = loadSession();
    if (!id || !session || settingsPlace() !== "group-member") return;
    let alive = true;
    serverFetch(session, "POST", `/api/rpc/${session.scope}/take_group_notes`, { characterId: id }).then(
      (lines) => { if (alive && Array.isArray(lines) && lines.length > 0) setNotes({ name: name.current, lines: lines.map(String) }); },
      () => { /* nothing to show then */ },
    );
    return () => { alive = false; };
  }, [id]);
  if (!notes) return null;
  return (
    <MessageBox
      title="This group's rules"
      message={
        <>
          <p>Your Dungeon Master's rules for this group changed {notes.name || "this character"}:</p>
          <ul>{notes.lines.map((line) => <li key={line}>{line}</li>)}</ul>
        </>
      }
      buttons={[{ id: "ok", label: "OK", variant: "primary" }]}
      onResult={() => setNotes(null)}
    />
  );
}
