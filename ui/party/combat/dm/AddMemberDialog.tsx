import { useState } from "react";
import { Button } from "../../../src/components/Button";
import { Dialog } from "../../../src/components/controls/Dialog";
import type { D20Input } from "./common";

/**
 * Add a late-joining party member (0.3.5, A2): a party character who isn't in this fight (joined the party after it
 * began, or was set absent) comes in with its initiative: the player's table roll typed first, or "Roll for me",
 * which the server rolls with the character's own bonus. The engine slots it into the order; the player's Play tab
 * shows the live combat from then on. Names only: nothing of the character's numbers reaches the DM.
 */
export function AddMemberDialog({ joinable, onAdd, onClose }: {
  joinable: { characterId: string; name: string }[];
  onAdd: (characterId: string, initiative: D20Input) => void;
  onClose: () => void;
}) {
  const [picked, setPicked] = useState<string | null>(joinable.length === 1 ? joinable[0]!.characterId : null);
  const [text, setText] = useState("");
  const total = /^-?\d+$/.test(text.trim()) ? Number(text.trim()) : null;
  const add = (input: D20Input) => { if (picked) onAdd(picked, input); };
  return (
    <Dialog title="Add a party member" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button icon="dice" disabled={!picked} onClick={() => add({ kind: "app", modifier: 0, mode: "normal" })}>Roll for me</Button>
        <Button variant="primary" disabled={!picked || total === null} onClick={() => { if (total !== null) add({ kind: "typed", total }); }}>Add to the fight</Button>
      </>}>
      <div className="cb-form" data-testid="add-member">
        <div className="cb-types" role="radiogroup" aria-label="Party member">
          {joinable.map((j) => (
            <button key={j.characterId} type="button" role="radio" aria-checked={picked === j.characterId}
              className={`cb-type${picked === j.characterId ? " on" : ""}`} onClick={() => setPicked(j.characterId)}>{j.name}</button>
          ))}
        </div>
        <form onSubmit={(e) => { e.preventDefault(); if (total !== null) add({ kind: "typed", total }); }}>
          <label className="cb-field">
            <span>Initiative: the player's table roll</span>
            <input className="input cb-roll-box" inputMode="numeric" autoFocus placeholder="Table roll" aria-label="Initiative: typed result"
              value={text} onChange={(e) => setText(e.target.value)} />
          </label>
        </form>
        <p className="cb-small muted">They join the turn order where their initiative puts them; their Play tab shows the fight.</p>
      </div>
    </Dialog>
  );
}
