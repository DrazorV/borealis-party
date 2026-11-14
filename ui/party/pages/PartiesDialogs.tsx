import { type FormEvent, useState } from "react";
import type { CharacterSummary } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Alert, EmptyState } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { NumericUpDown } from "../../src/components/controls/Controls";
import { errorMessage } from "../../src/state/useAsync";
import {
  CODE_DAYS, formatCode, inviteUsername, joinLink, makeJoinCode, MAX_CODE_USES, MAX_NAME, type JoinCode, type Party, type Ruleset,
} from "../api/overrides/parties";
import { EDITION_EFFECT, EditionChoice } from "./PartyEdition";

/** A party's name: typed for a new party, or a new one for a party. */
export function NameDialog({ title, action, initial = "", onSave, onClose }: {
  title: string; action: string; initial?: string; onSave: (name: string) => Promise<void>; onClose: () => void;
}) {
  const [name, setName] = useState(initial);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    const typed = name.trim();
    if (!typed) return;
    setBusy(true);
    setError("");
    try {
      await onSave(typed);
      onClose();
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };
  return (
    <Modal title={title} onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !name.trim()} onClick={() => void submit()}>{action}</Button>
      </>}>
      <form className="party-form" onSubmit={(e) => void submit(e)}>
        <label className="party-field">
          Party name
          <input className="input" aria-label="Party name" maxLength={MAX_NAME} value={name} onChange={(e) => setName(e.target.value)} required />
        </label>
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/**
 * A new party (0.3.2): its name and its edition. The edition is required, with no silent default: `initial` (the
 * edition of the DM's most recent character) is pre-selected when there is one, else nothing is.
 */
export function NewPartyDialog({ initial, onSave, onClose }: {
  initial: Ruleset | null; onSave: (name: string, ruleset: Ruleset) => Promise<void>; onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [ruleset, setRuleset] = useState<Ruleset | null>(initial);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const ready = Boolean(name.trim()) && ruleset !== null;
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!ready || !ruleset) return;
    setBusy(true);
    setError("");
    try {
      await onSave(name.trim(), ruleset);
      onClose();
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };
  return (
    <Modal title="New party" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !ready} onClick={() => void submit()}>Create</Button>
      </>}>
      <form className="party-form" onSubmit={(e) => void submit(e)}>
        <label className="party-field">
          Party name
          <input className="input" aria-label="Party name" maxLength={MAX_NAME} value={name} onChange={(e) => setName(e.target.value)} required />
        </label>
        <EditionChoice value={ruleset} onChange={setRuleset} />
        <p className="muted party-note">
          {ruleset === null ? "Pick the rules your players' characters use. " : ""}{EDITION_EFFECT} You can change it later.
        </p>
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/**
 * Invite players (plan 3.4): by exact username, or with a join code to post in the group's chat. A username invite is
 * always "sent": the server never says whether that account exists.
 */
export function InviteDialog({ party, onClose }: { party: Party; onClose: () => void }) {
  const [mode, setMode] = useState<"username" | "code">("username");
  const [username, setUsername] = useState("");
  const [uses, setUses] = useState(1);
  const [days, setDays] = useState(CODE_DAYS);
  const [sent, setSent] = useState("");
  const [code, setCode] = useState<JoinCode | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try { await work(); } catch (err) { setError(errorMessage(err)); } finally { setBusy(false); }
  };
  const sendInvite = (e?: FormEvent) => {
    e?.preventDefault();
    const name = username.trim();
    if (!name) return;
    void run(async () => {
      await inviteUsername(party.id, name);
      setSent(name);
      setUsername("");
    });
  };
  const makeCode = () => void run(async () => { setCode(await makeJoinCode(party.id, uses, days)); });
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch { setError("Copying isn't allowed here. Select the code and copy it."); }
  };

  return (
    <Modal title={`Invite to ${party.name}`} onClose={onClose} footer={<Button onClick={onClose}>Done</Button>}>
      <div className="party-form">
        <div className="party-segmented" role="tablist" aria-label="Invite by">
          <button type="button" role="tab" aria-selected={mode === "username"} className={`btn${mode === "username" ? " btn-primary" : ""}`} onClick={() => setMode("username")}>Username</button>
          <button type="button" role="tab" aria-selected={mode === "code"} className={`btn${mode === "code" ? " btn-primary" : ""}`} onClick={() => setMode("code")}>Join code</button>
        </div>
        {mode === "username" && (
          <form className="party-form" onSubmit={sendInvite}>
            <p className="muted">Type the player's exact username. They see the invite on their Parties page and pick a character.</p>
            <label className="party-field">
              Username
              <input className="input" aria-label="Username" autoCapitalize="none" spellCheck={false} autoComplete="off" value={username} onChange={(e) => setUsername(e.target.value)} required />
            </label>
            <Button type="submit" variant="primary" disabled={busy || !username.trim()}>Send invite</Button>
            {sent && <Alert tone="success">Invite sent to {sent}. If that account exists, they'll see it on their Parties page.</Alert>}
          </form>
        )}
        {mode === "code" && !code && (
          <div className="party-form">
            <p className="muted">Anyone with the code can join with one of their characters, until it runs out or expires.</p>
            <div className="party-numbers">
              <NumericUpDown label="Number of uses" value={uses} min={1} max={MAX_CODE_USES} onChange={setUses} />
              <NumericUpDown label="Days it works" value={days} min={1} max={CODE_DAYS} onChange={setDays} />
            </div>
            <Button variant="primary" disabled={busy} onClick={makeCode}>Make a join code</Button>
          </div>
        )}
        {mode === "code" && code && (
          <div className="party-form">
            <p className="muted">Share this now: it isn't shown again.</p>
            <output className="party-code" aria-label="Join code">{formatCode(code.code)}</output>
            <p className="muted party-note">
              {code.invite.usesLeft === 1 ? "One use" : `${code.invite.usesLeft ?? uses} uses`}, until {new Date(code.invite.expiresAt).toLocaleDateString()}.
            </p>
            <div className="party-inline">
              <Button icon="copy" onClick={() => void copy(formatCode(code.code))}>Copy code</Button>
              <Button icon="link" onClick={() => void copy(code.link ?? joinLink(code.code))}>Copy link</Button>
            </div>
            {copied && <Alert tone="success">Copied.</Alert>}
            <Button onClick={() => { setCode(null); setCopied(false); }}>Make another</Button>
          </div>
        )}
        {error && <Alert tone="danger">{error}</Alert>}
      </div>
    </Modal>
  );
}

/**
 * Pick the character that joins (plan D19: a character is in at most one party). Characters already in a party are
 * shown, but can't be picked.
 */
export function PickCharacterDialog({ title, characters, taken, onPick, onClose }: {
  title: string;
  characters: CharacterSummary[];
  /** Character id -> the name of the party it is in. */
  taken: Map<string, string>;
  onPick: (characterId: string) => Promise<void>;
  onClose: () => void;
}) {
  const free = characters.filter((c) => !taken.has(c.id));
  const [chosen, setChosen] = useState(free[0]?.id ?? "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const join = async () => {
    if (!chosen) return;
    setBusy(true);
    setError("");
    try {
      await onPick(chosen);
      onClose();
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };
  return (
    <Modal title={title} onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !chosen} onClick={() => void join()}>Join with this character</Button>
      </>}>
      <div className="party-form">
        <p className="muted">Your character stays yours. The DM sees a read-only card, and can't see your other characters.</p>
        {characters.length === 0 && <EmptyState title="No characters yet">Create one on the Start page first.</EmptyState>}
        {characters.length > 0 && free.length === 0 && <Alert tone="info">Each of your characters is already in a party. A character can be in one party at a time.</Alert>}
        {characters.length > 0 && (
          <ul className="party-list party-pick" role="radiogroup" aria-label="Character">
            {characters.map((c) => {
              const inParty = taken.get(c.id);
              return (
                <li key={c.id}>
                  <label className={`party-pick-row${inParty ? " disabled" : ""}${chosen === c.id ? " chosen" : ""}`}>
                    <input type="radio" name="party-character" value={c.id} checked={chosen === c.id} disabled={Boolean(inParty)} onChange={() => setChosen(c.id)} />
                    <span className="grow">
                      <strong>{c.name || "Unnamed"}</strong>
                      <span className="muted party-note">{c.build || `Level ${c.level}`}{inParty ? ` · in ${inParty}` : ""}</span>
                    </span>
                  </label>
                </li>
              );
            })}
          </ul>
        )}
        {error && <Alert tone="danger">{error}</Alert>}
      </div>
    </Modal>
  );
}
