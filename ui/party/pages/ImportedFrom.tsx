import { useId, useMemo, useState } from "react";
import type { FiveToolsUrl } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Icon } from "../../src/components/Icon";
import { matches, plural, stripFormat, usePref } from "../content/compact";

/**
 * "Imported from" in the Import section's 5etools card: each 5etools source on one line (its name, how many packs,
 * when it was last imported, Update), its packs folded under "Show packs" as a list of chips with a search once there
 * are many. The packs are named without the "5etools-format:" each one carries: the line says it once. Whether a
 * source's packs are shown is kept on this device (../content/compact.ts).
 */

/** From this many packs on, the list gets a search box. */
export const SEARCH_FROM = 12;

export function whenText(s: Pick<FiveToolsUrl, "lastImported" | "lastChecked">): string | null {
  const iso = s.lastImported ?? s.lastChecked;
  if (!iso) return null;
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? null : `updated ${d.toLocaleDateString(undefined, { dateStyle: "medium" })}`;
}

function Packs({ names }: { names: string[] }) {
  const [text, setText] = useState("");
  const shown = useMemo(() => names.filter((n) => matches(n, text)), [names, text]);
  const searchId = useId();
  return (
    <div className="party-import-packs">
      {names.length >= SEARCH_FROM && (
        <label className="party-compact-search" htmlFor={searchId}>
          <span className="sr-only">Search packs</span>
          <input id={searchId} className="input" type="search" placeholder="Search packs" autoComplete="off" spellCheck={false}
            value={text} onChange={(e) => setText(e.target.value)} />
        </label>
      )}
      {shown.length === 0
        ? <p className="muted small">No pack matches.</p>
        : <ul className="party-import-pack-chips" aria-label="Packs">{shown.map((n, i) => <li key={`${n}-${i}`} className="chip">{n}</li>)}</ul>}
    </div>
  );
}

function SourceRow({ source, title, packName, busy, onUpdate }: {
  source: FiveToolsUrl; title: string; packName: (id: string) => string; busy: boolean; onUpdate: () => void;
}) {
  const [open, setOpen] = usePref(`imported.${source.url}`, false);
  const listId = useId();
  const packs = source.packs ?? [];
  const names = useMemo(() => packs.map((id) => stripFormat(packName(id))), [packs, packName]);
  const when = whenText(source);
  const facts = ["5etools-format data", plural(packs.length, "pack"), ...(when ? [when] : [])].join(" · ");
  return (
    <li className="party-import-known-row">
      <div className="party-import-known-text">
        <strong className="party-import-url">{title}</strong>
        {title !== source.url && <span className="muted small party-import-url">{source.url}</span>}
        <span className="muted small">{facts}</span>
        {source.problem && <span className="small party-import-field-error">{source.problem}</span>}
      </div>
      <div className="party-import-known-acts">
        {packs.length > 0 && (
          <button type="button" className="party-compact-toggle" aria-expanded={open} aria-controls={listId} onClick={() => setOpen(!open)}>
            {open ? "Hide packs" : "Show packs"}<Icon name={open ? "up" : "down"} />
          </button>
        )}
        <Button disabled={busy} aria-label={`Update from ${title}`} onClick={onUpdate}>Update</Button>
      </div>
      {open && packs.length > 0 && <div id={listId} className="party-import-known-packs"><Packs names={names} /></div>}
    </li>
  );
}

export function ImportedFrom({ sources, packNames, title, busy, onUpdate }: {
  sources: FiveToolsUrl[]; packNames: Record<string, string>; title: (url: string) => string; busy: boolean; onUpdate: (url: string) => void;
}) {
  const packName = useMemo(() => (id: string) => packNames[id] ?? id, [packNames]);
  if (sources.length === 0) return null;
  return (
    <div className="party-import-known">
      <h3 id="party-5et-known">Imported from</h3>
      <ul aria-labelledby="party-5et-known">
        {sources.map((s) => (
          <SourceRow key={s.url} source={s} title={title(s.url)} packName={packName} busy={busy} onUpdate={() => onUpdate(s.url)} />
        ))}
      </ul>
    </div>
  );
}
