import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { Button, IconButton } from "../../src/components/Button";
import { Alert, Chip } from "../../src/components/Card";
import { errorMessage } from "../../src/state/useAsync";
import {
  EDITION_WORD, isNotFound, LIMITS, monsterFacets, searchMonsters, type Facets, type Kind, type MonsterQuery,
  type MonsterSummary,
} from "../api/overrides/encounters";
import type { Ruleset } from "../api/overrides/parties";
import { EncounterToolPanel, useWide } from "./EncounterToolPanel";

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
 * The creature picker (plan 4.3): a search box and filter chips (kind, edition, CR, type, legendary) over the DM's own
 * typed Monster elements and Aurora companions from the bestiary search (D17 b). `owner`, when given, is the creature
 * the picked ones will be controlled by (a pet, a summon).
 *
 * On a phone it is a full-screen sheet with a basket: tap + to pick, then add them all at once. On a big screen (0.3.1)
 * it is a panel beside the creatures that stays open while the DM adds many: the search has the focus, the arrow keys
 * move through the results, Enter adds the highlighted one ("How many" of it), and each row has its own Add.
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
  const wide = useWide();
  const [cursor, setCursor] = useState(0);
  const [howMany, setHowMany] = useState(1);
  const [added, setAdded] = useState("");
  const search = useRef<HTMLInputElement>(null);
  useEffect(() => { if (wide) search.current?.focus(); }, [wide, owner]);

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
      if (offset === 0) setCursor(0);
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
  /** Docked: add this one now and keep the panel open for the next. */
  const addNow = async (m: MonsterSummary, n: number) => {
    if (room <= 0 || busy) return;
    const want = Math.max(1, Math.min(n, LIMITS.addAtOnce, room));
    setBusy(true);
    try {
      await onAdd([{ monster: m, count: want }]);
      setAdded(want === 1 ? `Added ${m.name}.` : `Added ${want} × ${m.name}.`);
      setHowMany(1);
      setError("");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
      search.current?.focus();
    }
  };
  const onSearchKey = (e: KeyboardEvent) => {
    if (!wide) return;
    if (e.key === "ArrowDown") { e.preventDefault(); setCursor((c) => Math.min(items.length - 1, c + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setCursor((c) => Math.max(0, c - 1)); }
    else if (e.key === "Enter") {
      e.preventDefault();
      const m = items[cursor];
      if (m) void addNow(m, howMany);
    }
  };
  useEffect(() => {
    if (wide) document.getElementById(`enc-result-${cursor}`)?.scrollIntoView?.({ block: "nearest" });
  }, [cursor, wide]);
  const toggleKind = (k: Kind) => setKinds(kinds.includes(k) ? kinds.filter((x) => x !== k) : [...kinds, k]);
  const typeChips = (facets?.types ?? []).filter((t) => t.count > 0).map((t) => t.value);

  return (
    <EncounterToolPanel title={owner ? `Pets and summons for ${owner}` : "Add creatures"} className="enc-picker" onClose={onClose}
      footer={wide ? undefined : <>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || count === 0} onClick={() => void add()}>
          {count === 0 ? "Add" : count === 1 ? "Add 1 creature" : `Add ${count} creatures`}
        </Button>
      </>}>
      <div className="enc-picker-body">
        <div className={wide ? "enc-search-row" : "enc-search-wrap"}>
          <input ref={search} className="input enc-search" type="search" aria-label="Search creatures"
            placeholder={wide ? "Search your creatures (Enter adds)" : "Search your creatures"} value={text}
            onChange={(e) => setText(e.target.value)} onKeyDown={onSearchKey} autoComplete="off" spellCheck={false}
            {...(wide ? {
              role: "combobox", "aria-expanded": items.length > 0, "aria-controls": "enc-results",
              "aria-activedescendant": items[cursor] ? `enc-result-${cursor}` : undefined,
            } : {})} />
          {wide && (
            <label className="enc-howmany">
              <span className="muted party-note">How many</span>
              <input className="input" type="number" min={1} max={LIMITS.addAtOnce} value={howMany} aria-label="How many to add"
                onChange={(e) => setHowMany(Math.max(1, Math.min(LIMITS.addAtOnce, Number(e.target.value) || 1)))} onKeyDown={onSearchKey} />
            </label>
          )}
        </div>
        <div className="enc-filters">
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
          <div className={`enc-chips${wide ? "" : " enc-chips-scroll"}`} role="group" aria-label="Creature type">
            {typeChips.map((t) => (
              <ToggleChip key={t} on={types.includes(t)} label={cap(t)}
                onClick={() => setTypes(types.includes(t) ? types.filter((x) => x !== t) : [...types, t])} />
            ))}
          </div>
        )}
        </div>
        {error && <Alert tone="danger">{error}</Alert>}
        <p className="muted party-note" aria-live="polite">{loading ? "Searching…" : `${total} ${total === 1 ? "creature" : "creatures"}`}{room < LIMITS.creatures ? ` · room for ${room - count} more` : ""}</p>
        {wide && added && <p className="party-note enc-added" role="status">{added}</p>}
        <ul className="enc-results" id="enc-results" aria-label="Creatures" role={wide ? "listbox" : undefined}>
          {items.map((m, i) => {
            const have = basket.get(m.id)?.count ?? 0;
            return (
              <li key={m.id} id={`enc-result-${i}`} className={`enc-result ${have ? "picked" : ""} ${wide && i === cursor ? "is-cursor" : ""}`}
                {...(wide ? { role: "option", "aria-selected": i === cursor, "aria-label": m.name, onMouseEnter: () => setCursor(i) } : {})}>
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
                {wide ? (
                  <Button small icon="plus" disabled={busy || room <= 0} aria-label={`Add ${m.name}`} onClick={() => void addNow(m, howMany)}>Add</Button>
                ) : (
                  <span className="enc-stepper">
                    {have > 0 && <IconButton icon="minus" label={`One fewer ${m.name}`} onClick={() => change(m, -1)} />}
                    {have > 0 && <span className="enc-count" aria-label={`${have} picked`}>{have}</span>}
                    <IconButton icon="plus" label={`Pick ${m.name}`} disabled={count >= room || have >= LIMITS.addAtOnce} onClick={() => change(m, 1)} />
                  </span>
                )}
              </li>
            );
          })}
        </ul>
        {items.length < total && !loading && <Button onClick={() => void fetchPage(items.length)}>Show more</Button>}
      </div>
    </EncounterToolPanel>
  );
}
