import { useEffect, useId, useMemo, useState } from "react";
import { Button } from "../../src/components/Button";
import { Expander } from "../../src/components/controls";
import { Section } from "../../src/pages/content/Section";
import { useApi } from "../../src/state/ApiContext";
import { useAsync } from "../../src/state/useAsync";
import { bestiaryCoverage, type Count, type Coverage, type PackCoverage } from "../api/bestiary";
import { FILTERS, keep, pickSources, plural, type SourceFilter, type SourceSort, stripFormat, usePref } from "./compact";
import "./content.css";

/**
 * Bestiary coverage on Additional Content (0.3.0): for each installed source with creatures, how many stat blocks came
 * in, whether its bestiary came in fully, and what the 5etools import kept as notes (party/docs/bestiary-api.md).
 * Aurora content brings companions (familiars, beast companions, steeds) instead, counted beside them; both can be
 * picked for encounters (D17 b). Reloads with the content.
 * Since 0.3.1 the section is a summary line with the sources folded under "Show sources": a search, a status filter
 * and a sort above one line per source (SRD first), each opening to its details (./compact.ts keeps the choices).
 */

const STATUS_LABEL: Record<PackCoverage["status"], string> = {
  complete: "Complete",
  partial: "Partly imported",
  unknown: "Not checked",
  companions_only: "Companions only",
  installed: "Installed",
};

/** One line under the source's name. */
export function summaryLine(p: PackCoverage): string {
  switch (p.status) {
    case "companions_only":
      return `${plural(p.companions, "companion")} as Aurora lists them (no typed stat blocks).`;
    case "unknown":
      return `${plural(p.monsters, "creature")}. Import or update this source again to check it against the data.`;
    case "installed":
      return [plural(p.monsters, "creature"), ...(p.companions > 0 ? [plural(p.companions, "companion")] : [])].join(" · ");
    default: {
      const parts = [`${p.monsters.toLocaleString()} of ${plural(p.inData ?? p.monsters, "creature")}`];
      if (p.companions > 0) parts.push(plural(p.companions, "companion"));
      if (p.skipped > 0) parts.push(`${p.skipped.toLocaleString()} not imported`);
      if (p.partial > 0) parts.push(`${p.partial.toLocaleString()} with details kept as notes`);
      return parts.join(" · ");
    }
  }
}

const EXTRAS: [keyof PackCoverage["extras"], string][] = [
  ["environment", "with an environment"],
  ["page", "with a page"],
  ["mythic", "with mythic actions"],
  ["covenCr", "with a coven CR"],
  ["altSpeeds", "with another speed"],
  ["variants", "with a variant sidebar"],
  ["reactionIntro", "with a reactions introduction"],
  ["fluff", "with lore text in the data (not imported)"],
  ["tokens", "with a token in the data (not imported)"],
];

function Counts({ label, counts }: { label: string; counts: Count[] }) {
  if (counts.length === 0) return null;
  return (
    <div className="party-bestiary-counts" role="group" aria-label={label}>
      <p className="party-bestiary-label">{label}</p>
      <ul>{counts.map((c) => <li key={c.value} className="chip">{c.value} <span className="muted">{c.count}</span></li>)}</ul>
    </div>
  );
}

function Notes({ label, counts }: { label: string; counts: Count[] }) {
  if (counts.length === 0) return null;
  return (
    <div className="party-bestiary-notes" role="group" aria-label={label}>
      <p className="party-bestiary-label">{label}</p>
      <ul>{counts.map((c) => <li key={c.value}><span className="party-report-count">{c.count}</span> {c.value}</li>)}</ul>
    </div>
  );
}

/** The counts on the source's own line, short: the full sentence is inside its expander. */
export function shortLine(p: PackCoverage): string {
  switch (p.status) {
    case "companions_only": return plural(p.companions, "companion");
    case "unknown": return `${plural(p.monsters, "creature")}, not checked`;
    case "installed": return summaryLine(p);
    default: {
      const parts = [p.inData !== null && p.inData !== p.monsters ? `${p.monsters.toLocaleString()} of ${plural(p.inData, "creature")}` : plural(p.monsters, "creature")];
      if (p.companions > 0) parts.push(plural(p.companions, "companion"));
      if (p.skipped > 0) parts.push(`${p.skipped.toLocaleString()} not imported`);
      if (p.partial > 0) parts.push(`${p.partial.toLocaleString()} with notes`);
      return parts.join(" · ");
    }
  }
}

function PackRow({ p }: { p: PackCoverage }) {
  const extras = EXTRAS.filter(([k]) => p.extras[k] > 0).map(([k, words]) => ({ value: words, count: p.extras[k] }));
  const name = stripFormat(p.name);
  const title = (
    <>
      <strong className="party-bestiary-name">{name}</strong>
      <span className="party-bestiary-short">{shortLine(p)}</span>
      <span className={`chip party-bestiary-${p.status}`}>{STATUS_LABEL[p.status]}</span>
      {!p.enabled && <span className="chip">Turned off</span>}
    </>
  );
  return (
    <li className="party-bestiary-pack" aria-label={name}>
      <Expander defaultOpen={false} title={title}>
        <p className="muted small party-bestiary-line">{summaryLine(p)}</p>
        <Counts label="By challenge rating" counts={p.byCr} />
        <Counts label="By creature type" counts={p.byType} />
        <Notes label="Not imported" counts={p.skippedReasons} />
        <Notes label="Kept as notes" counts={p.partialNotes} />
        <Notes label="Extra details kept" counts={extras} />
      </Expander>
    </li>
  );
}

const SORTS: { value: SourceSort; label: string }[] = [
  { value: "name", label: "By name" },
  { value: "count", label: "Most creatures first" },
];
const isFilter = (v: string): v is SourceFilter => FILTERS.some((f) => f.value === v);
const isSort = (v: string): v is SourceSort => SORTS.some((s) => s.value === v);

/** The headline under the section title: how much is there, and how much of it came in only partly. */
export function totalsLine(c: Coverage): string {
  const partly = c.packs.filter((p) => p.status === "partial").length;
  const parts = [`${plural(c.totals.monsters, "creature")} with stat blocks${c.totals.companions > 0 ? ` and ${plural(c.totals.companions, "companion")}` : ""} in ${plural(c.packs.length, "source")}`];
  if (partly > 0) parts.push(`${partly.toLocaleString()} partly imported`);
  return `${parts.join(" · ")}. These can be picked for encounters.`;
}

function Sources({ packs }: { packs: PackCoverage[] }) {
  const [text, setText] = useState("");
  const [rawFilter, setFilter] = usePref<string>("bestiary.filter", "all");
  const [rawSort, setSort] = usePref<string>("bestiary.sort", "name");
  const filter = isFilter(rawFilter) ? rawFilter : "all";
  const sort = isSort(rawSort) ? rawSort : "name";
  const shown = useMemo(() => pickSources(packs, { text, filter, sort }), [packs, text, filter, sort]);
  const searchId = useId();
  return (
    <div className="party-compact-tools-wrap">
      <div className="party-compact-tools">
        <label className="party-compact-search" htmlFor={searchId}>
          <span className="sr-only">Search sources</span>
          <input id={searchId} className="input" type="search" placeholder="Search sources" autoComplete="off" spellCheck={false}
            value={text} onChange={(e) => setText(e.target.value)} />
        </label>
        <div className="party-compact-filter" role="group" aria-label="Filter sources">
          {FILTERS.map((f) => (
            <Button key={f.value} small variant={filter === f.value ? "primary" : "default"} className="party-compact-seg" aria-pressed={filter === f.value} onClick={() => setFilter(f.value)}>
              {f.label} <span className="party-compact-n">{packs.filter((p) => keep(p, f.value)).length}</span>
            </Button>
          ))}
        </div>
        <label className="party-compact-sort">
          <span className="sr-only">Sort sources</span>
          <select className="input" value={sort} onChange={(e) => setSort(e.target.value)}>
            {SORTS.map((s) => <option key={s.value} value={s.value}>{s.label}</option>)}
          </select>
        </label>
      </div>
      <p className="muted small party-compact-count" role="status">{shown.length === packs.length ? `All ${plural(packs.length, "source")}` : `${shown.length.toLocaleString()} of ${plural(packs.length, "source")}`}</p>
      {shown.length === 0
        ? <p className="muted">No source matches.</p>
        : <ul className="list party-bestiary" aria-label="Bestiary by source">{shown.map((p) => <PackRow key={p.packId} p={p} />)}</ul>}
    </div>
  );
}

export function BestiaryCoverage() {
  const api = useApi();
  const coverage = useAsync(() => bestiaryCoverage(), []);
  useEffect(() => api.subscribe("content:reloaded", coverage.reload), [api, coverage.reload]);
  const [open, setOpen] = usePref("bestiary.open", false);
  const packs = coverage.data?.packs ?? [];
  return (
    <Section title="Bestiary">
      {coverage.error && <p className="muted" role="alert">The bestiary report could not be loaded: {coverage.error}</p>}
      {coverage.loading && !coverage.data && <p className="muted">Counting creatures…</p>}
      {coverage.data && (
        <div className="party-compact">
          <p className="muted party-compact-summary">{totalsLine(coverage.data)}</p>
          {packs.length > 0 && (
            <Expander open={open} onToggle={setOpen} title={`${open ? "Hide" : "Show"} sources (${packs.length.toLocaleString()})`}>
              <Sources packs={packs} />
            </Expander>
          )}
        </div>
      )}
    </Section>
  );
}
