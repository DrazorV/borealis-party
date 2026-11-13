import { useCallback, useEffect, useState } from "react";
import type { ContentIndex, ContentPack } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import { IndexesTab as UpstreamIndexesTab, sourceConsentText } from "../../src/pages/content/IndexesTab";
import { Section } from "../../src/pages/content/Section";
import { useApi } from "../../src/state/ApiContext";
import { errorMessage } from "../../src/state/useAsync";
import { type PackSource, packSources } from "../api/packRemoval";
import "./content.css";

/**
 * Additional Content's index files, with a repair above them: the installed packs that came from an index this
 * account does not follow (they get no updates), each with "Add its index". This happens when a bundle brought a
 * pack whose index could not be added, or when the index was removed and its packs kept. Upstream's ContentPage
 * imports this file in place of its IndexesTab (ui/party/vite.config.ts). The server names an index that has not
 * been downloaded yet after its pack (party/crates/borealis-server/src/index_subs.rs).
 */

export interface Unfollowed { url: string; packs: string[] }

const trimmed = (url: string) => url.trim().replace(/\/+$/, "");

/** The index addresses packs came from that `indexes` lacks, each with the names of its packs, sorted by name. */
export function unfollowed(sources: PackSource[], indexes: ContentIndex[], packs: ContentPack[]): Unfollowed[] {
  const followed = new Set(indexes.map((i) => trimmed(i.url)));
  const names = new Map(packs.map((p) => [p.id, p.name]));
  const byUrl = new Map<string, string[]>();
  for (const s of sources) {
    if (!s.indexUrl || followed.has(trimmed(s.indexUrl))) continue;
    const list = byUrl.get(s.indexUrl) ?? [];
    list.push(names.get(s.id) ?? s.id);
    byUrl.set(s.indexUrl, list);
  }
  return [...byUrl].map(([url, list]) => ({ url, packs: list.sort() }))
    .sort((a, b) => a.packs[0]!.localeCompare(b.packs[0]!));
}

const hostOf = (url: string): string => { try { return new URL(url).host; } catch { return url; } };

export function IndexesTab() {
  const api = useApi();
  const confirm = useConfirm();
  const toast = useToast();
  const [rows, setRows] = useState<Unfollowed[]>([]);
  const [busy, setBusy] = useState(false);
  /** Upstream's list loads once; it is drawn again after an index is added here. */
  const [round, setRound] = useState(0);
  const load = useCallback(() => {
    void Promise.all([packSources(), api.listIndexes(), api.listPacks()])
      .then(([sources, indexes, packs]) => setRows(unfollowed(sources, indexes, packs)))
      .catch(() => setRows([]));
  }, [api]);
  useEffect(load, [load]);
  useEffect(() => api.subscribe("content:reloaded", load), [api, load]);

  const add = async (row: Unfollowed) => {
    const r = await confirm({ title: "Add content source", body: sourceConsentText(hostOf(row.url)), confirmLabel: "Add source" });
    if (r !== "confirm") return;
    setBusy(true);
    try {
      await api.addIndex({ url: row.url });
      toast.push(`Following ${row.packs.join(", ")}'s index. Press Update to check it for newer files.`, "info");
      setRound((n) => n + 1);
      load();
    } catch (e) {
      toast.push(errorMessage(e), "error");
    } finally { setBusy(false); }
  };

  return (
    <>
      {rows.length > 0 && (
        <Section title="Packs without their index">
          <p>These packs came from index files you don't follow, so they get no updates. Add the index to keep them up to date.</p>
          <ul className="list" aria-label="Packs without their index">
            {rows.map((row) => (
              <li key={row.url}><div className="inv-row party-unfollowed">
                <span style={{ flex: 1 }}><strong>{row.packs.join(", ")}</strong><span className="muted small" style={{ display: "block" }}>{row.url}</span></span>
                <Button small disabled={busy} aria-label={`Add the index of ${row.packs.join(", ")}`} onClick={() => void add(row)}>Add its index</Button>
              </div></li>
            ))}
          </ul>
        </Section>
      )}
      <UpstreamIndexesTab key={round} />
    </>
  );
}
