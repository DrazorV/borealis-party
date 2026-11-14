import { useEffect, useId, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Modal } from "../../src/components/Modal";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import {
  EDITION_LABEL, editionCounts, editionToSwitchTo, loadParty, otherEdition, RULESETS, setPartyEdition, type Party, type Ruleset,
} from "../api/overrides/parties";

/**
 * The party's edition (0.3.2, D23 b): picked when the party is made, changed by its DM, and offered as a one-tap
 * switch when most of its characters use the other one. Encounters always use the party's edition.
 */

/** What changing the edition does, said the same way everywhere. */
export const EDITION_EFFECT =
  "The party's encounters use its edition, and so do the combats you start from now on. A combat that is running keeps the rules it started with.";

/** "3 use 2014 rules, 1 uses 2024 rules", or null for a party with no readable characters. */
export function countsLine(party: Pick<Party, "members">): string | null {
  const counts = editionCounts(party.members);
  const parts = RULESETS.filter((r) => counts[r] > 0).map((r) => `${counts[r]} ${counts[r] === 1 ? "uses" : "use"} ${EDITION_LABEL[r]}`);
  return parts.length ? `Its characters: ${parts.join(", ")}.` : null;
}

/** The two editions as a radio group; `value` null means none is picked yet (no silent default). */
export function EditionChoice({ value, onChange, label = "Edition" }: {
  value: Ruleset | null; onChange: (r: Ruleset) => void; label?: string;
}) {
  const id = useId();
  return (
    <div className="party-field">
      <span id={id}>{label}</span>
      <div className="segmented party-edition-choice" role="radiogroup" aria-labelledby={id} aria-required="true">
        {RULESETS.map((r) => (
          <button key={r} type="button" role="radio" aria-checked={value === r} onClick={() => onChange(r)}>{EDITION_LABEL[r]}</button>
        ))}
      </div>
    </div>
  );
}

/** The party as its page shows it, read once; `given` skips the read. */
function useParty(partyId: string, given?: Party | null): { party: Party | null; error: string } {
  const [party, setParty] = useState<Party | null>(given ?? null);
  const [error, setError] = useState("");
  useEffect(() => {
    if (given) { setParty(given); return; }
    let live = true;
    loadParty(partyId).then((p) => { if (live) setParty(p); }, (e: unknown) => { if (live) setError(errorMessage(e)); });
    return () => { live = false; };
  }, [partyId, given]);
  return { party, error };
}

/** The DM changes a party's edition: an app dialog saying what follows it, then the PATCH. */
export function ChangeEditionDialog({ partyId, party: given, onClose, onChanged }: {
  partyId: string; party?: Party | null; onClose: () => void; onChanged?: (party: Party) => void;
}) {
  const toast = useToast();
  const { party, error: loadError } = useParty(partyId, given);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const to = party ? otherEdition(party.ruleset) : null;
  const save = async () => {
    if (!party || !to) return;
    setBusy(true);
    setError("");
    try {
      const fresh = await setPartyEdition(party.id, to);
      toast.push(`${party.name} now uses ${EDITION_LABEL[to]}.`, "info");
      onChanged?.(fresh);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };
  const counts = party ? countsLine(party) : null;
  return (
    <Modal title="Change edition" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !to} onClick={() => void save()}>{to ? `Switch to ${EDITION_LABEL[to]}` : "Switch"}</Button>
      </>}>
      <div className="party-form">
        {party && <p><strong>{party.name}</strong> uses <strong>{EDITION_LABEL[party.ruleset]}</strong>.</p>}
        {!party && !loadError && <p className="muted">Loading…</p>}
        {counts && <p className="muted">{counts}</p>}
        <p>{EDITION_EFFECT}</p>
        {(loadError || error) && <Alert tone="danger">{loadError || error}</Alert>}
      </div>
    </Modal>
  );
}

/**
 * In the encounter's edition notes: a one-tap "Switch party to 2014 rules" (or 2024) for the DM when most of the
 * party's characters use the other edition, or nothing. The switch asks first.
 */
export function EditionSwitch({ partyId, onChanged }: { partyId: string; onChanged?: () => void }) {
  const confirm = useConfirm();
  const toast = useToast();
  const { party } = useParty(partyId);
  const [busy, setBusy] = useState(false);
  const to = party ? editionToSwitchTo(party) : null;
  const switchParty = async () => {
    if (!party || !to) return;
    const answer = await confirm({
      title: `Switch ${party.name} to ${EDITION_LABEL[to]}?`, confirmLabel: `Switch to ${EDITION_LABEL[to]}`,
      body: EDITION_EFFECT,
    });
    if (answer !== "confirm") return;
    setBusy(true);
    try {
      await setPartyEdition(party.id, to);
      toast.push(`${party.name} now uses ${EDITION_LABEL[to]}.`, "info");
      onChanged?.();
    } catch (e) {
      toast.push(errorMessage(e), "error");
      setBusy(false);
    }
  };
  if (!party || !to) return null;
  return (
    <div className="party-form party-edition-fix">
      <p>Most of the party's characters use {EDITION_LABEL[to]}. {countsLine(party)}</p>
      <Button variant="primary" disabled={busy} onClick={() => void switchParty()}>Switch party to {EDITION_LABEL[to]}</Button>
    </div>
  );
}

/** The encounter Setup's rules line: the party's edition, and for its DM a way to change it. */
export function PartyEditionRow({ partyId, ruleset, onChanged }: { partyId: string; ruleset: Ruleset; onChanged?: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="enc-setting-row party-edition-row">
      <span className="enc-setting-label">Rules</span>
      <span className="grow">The party's edition: <strong>{EDITION_LABEL[ruleset]}</strong></span>
      <Button small onClick={() => setOpen(true)}>Change edition</Button>
      {open && <ChangeEditionDialog partyId={partyId} onClose={() => setOpen(false)} onChanged={() => onChanged?.()} />}
    </div>
  );
}
