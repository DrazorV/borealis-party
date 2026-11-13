import { useEffect } from "react";
import { Expander } from "../../src/components/controls";
import { Section } from "../../src/pages/content/Section";
import { useApi } from "../../src/state/ApiContext";
import { useAsync } from "../../src/state/useAsync";
import { bestiaryCoverage, type Count, type PackCoverage } from "../api/bestiary";
import "./content.css";

/**
 * Bestiary coverage on Additional Content (0.3.0): for each installed source with creatures, how many stat blocks came
 * in, whether its bestiary came in fully, and what the 5etools import kept as notes (party/docs/bestiary-api.md).
 * Aurora content brings companions (familiars, beast companions, steeds) instead, counted beside them; both can be
 * picked for encounters (D17 b). Reloads with the content.
 */

const plural = (n: number, one: string, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

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

function PackRow({ p }: { p: PackCoverage }) {
  const extras = EXTRAS.filter(([k]) => p.extras[k] > 0).map(([k, words]) => ({ value: words, count: p.extras[k] }));
  const title = <><strong>{p.name}</strong> <span className={`chip party-bestiary-${p.status}`}>{STATUS_LABEL[p.status]}</span>{!p.enabled && <span className="chip">Turned off</span>}</>;
  return (
    <li className="party-bestiary-pack" aria-label={p.name}>
      <Expander defaultOpen={false} title={title}>
        <Counts label="By challenge rating" counts={p.byCr} />
        <Counts label="By creature type" counts={p.byType} />
        <Notes label="Not imported" counts={p.skippedReasons} />
        <Notes label="Kept as notes" counts={p.partialNotes} />
        <Notes label="Extra details kept" counts={extras} />
      </Expander>
      <p className="muted small party-bestiary-line">{summaryLine(p)}</p>
    </li>
  );
}

export function BestiaryCoverage() {
  const api = useApi();
  const coverage = useAsync(() => bestiaryCoverage(), []);
  useEffect(() => api.subscribe("content:reloaded", coverage.reload), [api, coverage.reload]);
  const packs = coverage.data?.packs ?? [];
  return (
    <Section title="Bestiary">
      {coverage.error && <p className="muted" role="alert">The bestiary report could not be loaded: {coverage.error}</p>}
      {coverage.loading && !coverage.data && <p className="muted">Counting creatures…</p>}
      {coverage.data && (
        <>
          <p className="muted">{plural(coverage.data.totals.monsters, "creature")} with stat blocks{coverage.data.totals.companions > 0 ? ` and ${plural(coverage.data.totals.companions, "companion")}` : ""} in {plural(packs.length, "source")}. These can be picked for encounters.</p>
          <ul className="list party-bestiary" aria-label="Bestiary by source">{packs.map((p) => <PackRow key={p.packId} p={p} />)}</ul>
        </>
      )}
    </Section>
  );
}
