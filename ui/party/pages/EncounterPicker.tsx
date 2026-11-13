import { useEffect, useMemo, useRef, useState } from "react";
import { Button, IconButton } from "../../src/components/Button";
import { Alert, Chip } from "../../src/components/Card";
import { Modal } from "../../src/components/Modal";
import { errorMessage } from "../../src/state/useAsync";
import {
  EDITION_WORD, isNotFound, LIMITS, monsterFacets, searchMonsters, type Facets, type Kind, type MonsterQuery,
  type MonsterSummary,
} from "../api/overrides/encounters";
import type { Ruleset } from "../api/overrides/parties";

/** The CR bands the picker offers as chips. CR is only a printed stat and a filter here (D16). */
export const CR_BANDS: { id: string; label: string; crMin: string; crMax: string }[] = [
  { id: "tiny", label: "CR 0–1/2", crMin: "0", crMax: "1/2" },
  { id: "low", label: "CR 1–4", crMin: "1", crMax: "4" },
  { id: "mid", label: "CR 5–10", crMin: "5", crMax: "10" },
  { id: "high", label: "CR 11–16", crMin: "11", crMax: "16" },
  { id: "epic", label: "CR 17+", crMin: "17", crMax: "30" },
];

const PAGE = 30;
const cap = (s: string) => (s ? s[0]!.toUpperCase() + s.slice(1) : s);

/** One chip that toggles. */
function ToggleChip({ on, label, onClick }: { on: boolean; label: string; onClick: () => void }) {
  return (
    <button type="button" className={`enc-chip ${on ? "on" : ""}`} aria-pressed={on} onClick={onClick}>{label}</button>
  );
}

/**
 * The creature picker (plan 4.3 phone spec): a full-screen sheet over the builder with a search box and filter chips
 * (kind, edition, CR, type, legendary), the DM's own typed Monster elements and Aurora companions from the bestiary
 * search (D17 b), and a basket: tap + to pick, then add them all at once. `owner`, when given, is the creature the
 * picked ones will be controlled by (a pet, a summon).
 */
export function EncounterPicker({ ruleset, room, owner, onAdd, onClose }: {
  ruleset: Ruleset;
  /** Creatures the encounter can still take. */
  room: number;
  owner?: string;
  onAdd: (picked: { monster: MonsterSummary; count: number }[]) => Promise<void>;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [edition, setEdition] = useState<Ruleset | "">(owner ? "" : ruleset);
  const [kinds, setKinds] = useState<Kind[]>([]);
  const [band, setBand] = useState<string>("");
  const [types, setTypes] = useState<string[]>([]);
  const [legendary, setLegendary] = useState(false);
  const [facets, setFacets] = useState<Facets | null>(null);
  const [items, setItems] = useState<MonsterSummary[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [basket, setBasket] = useState<Map<string, { monster: MonsterSummary; count: number }>>(new Map());
  const [busy, setBusy] = useState(false);
  const asked = useRef(0);

  useEffect(() => { monsterFacets().then(setFacets, () => setFacets(null)); }, []);

  const query = useMemo((): MonsterQuery => {
    const cr = CR_BANDS.find((b) => b.id === band);
    return {
      ...(kinds.length ? { kinds } : {}),
      ...(text.trim() ? { text: text.trim().slice(0, 100) } : {}),
      ...(edition ? { ruleset: edition } : {}),
      ...(cr ? { crMin: cr.crMin, crMax: cr.crMax } : {}),
      ...(types.length ? { types } : {}),
      ...(legendary ? { legendary: true } : {}),
      sort: "name",
      limit: PAGE,
    };
  }, [text, edition, kinds, band, types, legendary]);

  const fetchPage = async (offset: number) => {
    const ticket = ++asked.current;
    setLoading(true);
    try {
      const page = await searchMonsters({ ...query, offset });
      if (ticket !== asked.current) return;
      setItems((old) => (offset === 0 ? page.items : [...old, ...page.items]));
      setTotal(page.total);
      setError("");
    } catch (e) {
      if (ticket !== asked.current) return;
      setError(isNotFound(e) ? "This server can't search creatures yet." : errorMessage(e));
    } finally {
      if (ticket === asked.current) setLoading(false);
    }
  };
  // Typing waits a moment; chips search at once.
  useEffect(() => {
    const timer = setTimeout(() => { void fetchPage(0); }, text ? 250 : 0);
    return () => clearTimeout(timer);
  }, [query]);

  const picked = [...basket.values()];
  const count = picked.reduce((n, p) => n + p.count, 0);
  const change = (m: MonsterSummary, by: number) => setBasket((old) => {
    const next = new Map(old);
    const have = next.get(m.id)?.count ?? 0;
    const want = Math.max(0, Math.min(LIMITS.addAtOnce, have + by, room - count + have));
    if (want === 0) next.delete(m.id);
    else next.set(m.id, { monster: m, count: want });
    return next;
  });
  const add = async () => {
    setBusy(true);
    try {
      await onAdd(picked);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };
  const toggleKind = (k: Kind) => setKinds(kinds.includes(k) ? kinds.filter((x) => x !== k) : [...kinds, k]);
  const typeChips = (facets?.types ?? []).filter((t) => t.count > 0).map((t) => t.value);

  return (
    <Modal title={owner ? `Pets and summons for ${owner}` : "Add creatures"} full className="enc-picker" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || count === 0} onClick={() => void add()}>
          {count === 0 ? "Add" : count === 1 ? "Add 1 creature" : `Add ${count} creatures`}
        </Button>
      </>}>
      <div className="enc-picker-body">
        <input className="input enc-search" type="search" aria-label="Search creatures" placeholder="Search your creatures" value={text}
          onChange={(e) => setText(e.target.value)} autoComplete="off" spellCheck={false} />
        <div className="enc-chips" role="group" aria-label="Kind">
          <ToggleChip on={kinds.includes("monster")} label="Monsters" onClick={() => toggleKind("monster")} />
          <ToggleChip on={kinds.includes("companion")} label="Companions" onClick={() => toggleKind("companion")} />
          <ToggleChip on={legendary} label="Legendary" onClick={() => setLegendary(!legendary)} />
        </div>
        <div className="enc-chips" role="group" aria-label="Edition">
          {(["dnd2024", "dnd2014"] as Ruleset[]).map((r) => (
            <ToggleChip key={r} on={edition === r} label={`${EDITION_WORD[r]} rules`} onClick={() => setEdition(edition === r ? "" : r)} />
          ))}
        </div>
        <div className="enc-chips" role="group" aria-label="Challenge rating">
          {CR_BANDS.map((b) => <ToggleChip key={b.id} on={band === b.id} label={b.label} onClick={() => setBand(band === b.id ? "" : b.id)} />)}
        </div>
        {typeChips.length > 0 && (
          <div className="enc-chips enc-chips-scroll" role="group" aria-label="Creature type">
            {typeChips.map((t) => (
              <ToggleChip key={t} on={types.includes(t)} label={cap(t)}
                onClick={() => setTypes(types.includes(t) ? types.filter((x) => x !== t) : [...types, t])} />
            ))}
          </div>
        )}
        {error && <Alert tone="danger">{error}</Alert>}
        <p className="muted party-note" aria-live="polite">{loading ? "Searching…" : `${total} ${total === 1 ? "creature" : "creatures"}`}{room < LIMITS.creatures ? ` · room for ${room - count} more` : ""}</p>
        <ul className="enc-results" aria-label="Creatures">
          {items.map((m) => {
            const have = basket.get(m.id)?.count ?? 0;
            return (
              <li key={m.id} className={`enc-result ${have ? "picked" : ""}`}>
                <span className="grow enc-result-text">
                  <strong>{m.name}</strong>
                  <span className="muted party-note">
                    {[m.cr != null ? `CR ${m.cr}` : null, [m.size.map(cap).join("/"), m.type].filter(Boolean).join(" ") || null,
                      m.ac != null ? `AC ${m.ac}` : null, m.hp != null ? `HP ${m.hp}` : null].filter(Boolean).join(" · ")}
                  </span>
                  <span className="muted party-note">{m.source}{m.ruleset !== ruleset && EDITION_WORD[m.ruleset] ? ` · ${EDITION_WORD[m.ruleset]} rules` : ""}</span>
                </span>
                {m.kind === "companion" && <Chip tone="info">Companion</Chip>}
                {m.legendary && <Chip tone="gold">Legendary</Chip>}
                <span className="enc-stepper">
                  {have > 0 && <IconButton icon="minus" label={`One fewer ${m.name}`} onClick={() => change(m, -1)} />}
                  {have > 0 && <span className="enc-count" aria-label={`${have} picked`}>{have}</span>}
                  <IconButton icon="plus" label={`Pick ${m.name}`} disabled={count >= room || have >= LIMITS.addAtOnce} onClick={() => change(m, 1)} />
                </span>
              </li>
            );
          })}
        </ul>
        {items.length < total && !loading && <Button onClick={() => void fetchPage(items.length)}>Show more</Button>}
      </div>
    </Modal>
  );
}
