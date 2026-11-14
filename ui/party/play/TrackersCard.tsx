import { type FormEvent, useEffect, useState } from "react";
import type { PlayTracker, PlayTrackerReset } from "../../src/api/contract";
import { Button, Check, EmptyState, IconButton, SelectField, TextField } from "../../src/components";
import { Dialog, Expander } from "../../src/components/controls";
import { cleanTrackerName } from "../../src/lib/trackerName";
import { AmountControl } from "../../src/pages/play/AmountControl";
import { PIP_LIMIT, PipButtons, UsesLeft } from "../../src/pages/play/PipButtons";
import { usePlayMutate } from "../../src/pages/play/playQueue";
import { MAX_TRACKER_MAX, MAX_TRACKER_NAME, type TrackerProblem, trackerError } from "../../src/pages/play/TrackersCard";
import { useCharacter } from "../../src/state/CharacterContext";
import "../../src/pages/play/spell-tracker.css";
import "./quick-wins.css";

export * from "../../src/pages/play/TrackersCard";

/**
 * Upstream's Trackers card (`src/pages/play/TrackersCard.tsx`: the user's own counters, saved on the character, with
 * pips or -/+, any typed amount, rename, maximum and reset in the edit dialog), with quick counters (0.3.5, roadmap P5):
 * the empty card and the Add dialog offer common ones (hero points, luck, wand charges) that fill the form in one tap,
 * still changeable before Save. Nothing needs XML. The rows and the dialog are upstream's own, kept the same.
 */

const CONFIRM_MS = 4000;

const RESET_LABEL: Record<PlayTrackerReset, string> = { short_rest: "Short Rest", long_rest: "Long Rest", never: "No reset" };
const RESET_OPTIONS: { value: PlayTrackerReset; label: string }[] = [
  { value: "short_rest", label: "Short Rest" }, { value: "long_rest", label: "Long Rest" }, { value: "never", label: "Never" },
];

export interface TrackerPreset { name: string; max: number; reset: PlayTrackerReset }

/** Counters players most often keep by hand. Generic names only: a table renames them to taste. */
export const TRACKER_PRESETS: readonly TrackerPreset[] = [
  { name: "Hero points", max: 5, reset: "never" },
  { name: "Luck", max: 3, reset: "long_rest" },
  { name: "Wand charges", max: 7, reset: "long_rest" },
];

function trackerNames(t: PlayTracker): { spend: string; restore: string } {
  if (t.countsUp) {
    const state = `${t.used} of ${t.max}`;
    return { spend: `Add one to ${t.name}, ${state}`, restore: `Remove one from ${t.name}, ${state}` };
  }
  const state = `${t.max - t.used} of ${t.max} left`;
  return { spend: `Use one ${t.name}, ${state}`, restore: `Restore one ${t.name}, ${state}` };
}

function TrackerAmount({ t }: { t: PlayTracker }) {
  const run = usePlayMutate();
  const left = t.max - t.used;
  const spend = (n: number) => void run((api, id) => api.spendTracker({ characterId: id, trackerId: t.id, uses: Math.min(n, left) }));
  const restore = (n: number) => void run((api, id) => api.restoreTracker({ characterId: id, trackerId: t.id, uses: Math.min(n, t.used) }));
  const [spendLabel, restoreLabel] = t.countsUp ? ["Add", "Remove"] : ["Use", "Restore"];
  return (
    <li className="v5-line hp-amount-line">
      <AmountControl label={`${t.name} amount`} max={MAX_TRACKER_MAX} actions={[
        { label: spendLabel, name: `${spendLabel} an amount of ${t.name}`, run: spend, disabled: left <= 0 },
        { label: restoreLabel, name: `${restoreLabel} an amount of ${t.name}`, run: restore, disabled: t.used <= 0 },
      ]} />
    </li>
  );
}

function TrackerRow({ t, onEdit }: { t: PlayTracker; onEdit: () => void }) {
  const run = usePlayMutate();
  const [amount, setAmount] = useState(false);
  const spend = () => void run((api, id) => api.spendTracker({ characterId: id, trackerId: t.id }));
  const restore = () => void run((api, id) => api.restoreTracker({ characterId: id, trackerId: t.id }));
  const names = trackerNames(t);
  const meta = RESET_LABEL[t.reset] ?? t.reset;
  return (<>
    <li className="v5-line st-line">
      <span className="v5-row static grow"><span className="grow">{t.name}</span><span className="meta">{t.countsUp ? `${meta} · counts up` : meta}</span></span>
      {t.max <= PIP_LIMIT
        ? <PipButtons label={t.name} max={t.max} used={t.used} countsUp={t.countsUp} spendName={names.spend} restoreName={names.restore} onSpend={spend} onRestore={restore} />
        : t.countsUp
          ? <span className="v5-ctrls st-steps">
            <IconButton icon="minus" variant="default" label={`Remove one from ${t.name}`} disabled={t.used <= 0} onClick={restore} />
            <IconButton icon="plus" variant="default" label={`Add one to ${t.name}`} disabled={t.used >= t.max} onClick={spend} />
          </span>
          : <span className="v5-ctrls st-steps">
            <IconButton icon="minus" variant="default" label={`Spend ${t.name}`} disabled={t.used >= t.max} onClick={spend} />
            <IconButton icon="plus" variant="default" label={`Restore ${t.name}`} disabled={t.used <= 0} onClick={restore} />
          </span>}
      <UsesLeft name={t.name} max={t.max} used={t.used} countsUp={t.countsUp} />
      <span className="v5-ctrls">
        <IconButton icon="more" label={`Change ${t.name} by an amount`} aria-expanded={amount} onClick={() => setAmount(!amount)} />
        <IconButton icon="pen" label={`Edit ${t.name}`} onClick={onEdit} />
      </span>
    </li>
    {amount && <TrackerAmount t={t} />}
  </>);
}

function DeleteButton({ disabled, onDelete }: { disabled: boolean; onDelete: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return undefined;
    const t = window.setTimeout(() => setArmed(false), CONFIRM_MS);
    return () => window.clearTimeout(t);
  }, [armed]);
  return (
    <Button variant="danger" disabled={disabled} onClick={() => (armed ? onDelete() : setArmed(true))}>
      {armed ? "Confirm delete" : "Delete"}
    </Button>
  );
}

/** The quick counters, as small buttons that fill the form (or open it filled). */
function Presets({ onPick, existing }: { onPick: (p: TrackerPreset) => void; existing: readonly string[] }) {
  const taken = new Set(existing.map((n) => n.toLowerCase()));
  const free = TRACKER_PRESETS.filter((p) => !taken.has(p.name.toLowerCase()));
  if (free.length === 0) return null;
  return (
    <div className="bp-presets" role="group" aria-label="Quick counters">
      <span className="bp-presets-label">Quick:</span>
      {free.map((p) => <Button key={p.name} small icon="plus" onClick={() => onPick(p)}>{p.name}</Button>)}
    </div>
  );
}

function TrackerDialog({ tracker, preset, existing, onClose }: { tracker: PlayTracker | null; preset: TrackerPreset | null; existing: readonly string[]; onClose: () => void }) {
  const { mutate, busy } = useCharacter();
  const [name, setName] = useState(tracker?.name ?? preset?.name ?? "");
  const [max, setMax] = useState(String(tracker?.max ?? preset?.max ?? 1));
  const [reset, setReset] = useState<PlayTrackerReset>(tracker?.reset ?? preset?.reset ?? "long_rest");
  const [countsUp, setCountsUp] = useState(tracker?.countsUp ?? false);
  const [error, setError] = useState<TrackerProblem | null>(null);
  const pick = (p: TrackerPreset) => { setName(p.name); setMax(String(p.max)); setReset(p.reset); setError(null); };
  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    const problem = trackerError(name, max);
    setError(problem);
    if (problem) return;
    const fields = { name: cleanTrackerName(name), max: Number(max), reset, countsUp };
    const done = await mutate((api, id) => (tracker
      ? api.updateTracker({ characterId: id, trackerId: tracker.id, ...fields })
      : api.addTracker({ characterId: id, ...fields })));
    if (done) onClose();
  };
  const remove = async () => {
    if (!tracker) return;
    if (await mutate((api, id) => api.removeTracker({ characterId: id, trackerId: tracker.id }))) onClose();
  };
  const errorOf = (field: TrackerProblem["field"]) => (error?.field === field ? error.message : null);
  return (
    <Dialog title={tracker ? "Edit tracker" : "Add tracker"} onClose={onClose} footer={<>
      {tracker && <DeleteButton disabled={busy} onDelete={() => void remove()} />}
      <Button onClick={onClose}>Cancel</Button>
      <Button variant="primary" disabled={busy} onClick={() => void save()}>Save</Button>
    </>}>
      <form className="st-form" onSubmit={(e) => void save(e)} noValidate>
        {!tracker && <Presets onPick={pick} existing={existing} />}
        <TextField label="Name" value={name} maxLength={MAX_TRACKER_NAME} autoComplete="off" error={errorOf("name")} onChange={(e) => setName(e.target.value)} />
        <TextField label="Maximum" type="number" inputMode="numeric" min={1} max={MAX_TRACKER_MAX} value={max} error={errorOf("max")} onChange={(e) => setMax(e.target.value)} />
        <SelectField label="Resets on" value={reset} onChange={(v) => setReset(v as PlayTrackerReset)} options={RESET_OPTIONS} />
        <Check label="Counts up" description="Show how many are marked (such as doom marks gained) instead of how many are left."
          checked={countsUp} onChange={(e) => setCountsUp(e.target.checked)} />
        <button type="submit" hidden aria-hidden="true" tabIndex={-1} />
      </form>
    </Dialog>
  );
}

type Editing = { tracker: PlayTracker | null; preset: TrackerPreset | null } | null;

export function TrackersCard({ trackers }: { trackers: PlayTracker[] }) {
  const [editing, setEditing] = useState<Editing>(null);
  const names = trackers.map((t) => t.name);
  return (
    <Expander title="Trackers" headerExtra={<Button small icon="plus" aria-label="Add tracker" onClick={() => setEditing({ tracker: null, preset: null })}>Add</Button>}>
      <ul className="v5-list" aria-label="Trackers">
        {trackers.length === 0
          ? <li><EmptyState title="No trackers">Add a counter for anything else you count, such as hero points, luck or wand charges.</EmptyState>
            <Presets existing={names} onPick={(p) => setEditing({ tracker: null, preset: p })} /></li>
          : trackers.map((t) => <TrackerRow key={t.id} t={t} onEdit={() => setEditing({ tracker: t, preset: null })} />)}
      </ul>
      {editing && <TrackerDialog tracker={editing.tracker} preset={editing.preset} existing={names} onClose={() => setEditing(null)} />}
    </Expander>
  );
}
