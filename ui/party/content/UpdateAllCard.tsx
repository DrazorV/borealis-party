import { useCallback, useEffect, useState } from "react";
import type { ContentProgress } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { ProgressBar } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Section } from "../../src/pages/content/Section";
import { useApi } from "../../src/state/ApiContext";
import { errorMessage } from "../../src/state/useAsync";
import { lastCheckedText, updateAll, updateInfo, type UpdateAll, type UpdateInfo } from "../api/contentUpdate";
import { contentSync } from "./contentSync";
import "./content.css";

/**
 * "Update content" at the top of the installed content (0.3.3): ONE button updates the index files and every 5etools
 * source, with one progress line and one summary. It replaces upstream's "Update content files" section and its
 * duplicate list of 5etools addresses (hidden by ./content.css); each 5etools source keeps a small Update and Remove in
 * the Import section's 5etools card. The account's own "Automatic Updates" (upstream's `autoContentUpdate`, Settings,
 * Data files, on by default) has the server check at most once a day when the account's workspace opens.
 */
export function UpdateAllCard() {
  const api = useApi();
  const confirm = useConfirm();
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<ContentProgress | null>(null);
  const [result, setResult] = useState<UpdateAll | null>(null);
  const [error, setError] = useState("");

  const reload = useCallback(() => { void updateInfo().then(setInfo, () => undefined); }, []);
  useEffect(reload, [reload]);
  // Sources added or removed anywhere (this page, another tab or device): the counts change.
  useEffect(() => contentSync.subscribe(reload), [reload]);
  useEffect(() => api.subscribe("content:progress", (p) => { if (busy) setProgress(p); }), [api, busy]);

  const run = async () => {
    setBusy(true);
    setError("");
    setResult(null);
    setProgress(null);
    try {
      setResult(await updateAll());
      contentSync.changed();
    } catch (e) { setError(errorMessage(e)); }
    finally { setBusy(false); setProgress(null); reload(); }
  };

  const clear = async () => {
    const r = await confirm({
      title: "Clear downloaded content",
      body: "Delete every file downloaded from your index files? Your index files stay listed (Update all downloads them again), and your own imports and 5etools packs are kept.",
      confirmLabel: "Clear", danger: true,
    });
    if (r !== "confirm") return;
    try { await api.clearDownloadedContent(); contentSync.changed(); } catch (e) { setError(errorMessage(e)); }
    reload();
  };

  const nothing = info !== null && info.indexes === 0 && info.sources === 0;
  const last = result ?? info?.last ?? null;
  const checked = lastCheckedText(last?.checkedAt);
  return (
    <Section title="Update content">
      <div className="party-update-all">
        <div className="party-update-line">
          <span className="party-update-text" role="status">
            {busy
              ? (progress?.message || "Checking for content updates…")
              : checked ? `${checked} · ${last!.message}` : nothing ? "Add an index file or a 5etools source, then update them here." : "Not checked yet."}
          </span>
          <Button variant="primary" icon="download" disabled={busy || nothing} onClick={() => void run()}>Update all content</Button>
        </div>
        {busy && progress && progress.total > 0 && <ProgressBar label="Update progress" value={progress.done} max={Math.max(1, progress.total)} />}
        {!busy && last && last.failed.length > 0 && (
          <ul className="party-update-failed" aria-label="Not updated">
            {last.failed.map((f) => <li key={f} className="small">{f}</li>)}
          </ul>
        )}
        {error && <p className="party-import-field-error" role="alert">{error}</p>}
        <div className="party-update-opts">
          <span className="muted small party-update-auto">
            {info && (info.auto
              ? `Automatic Updates is on: checked when you open Borealis Party, at most once every ${info.everyHours} hours.`
              : "Automatic Updates is off.")} Change it under Settings, Data files.
          </span>
          <Button small disabled={busy || !info || info.indexes === 0} onClick={() => void clear()}>Clear downloaded content</Button>
        </div>
      </div>
    </Section>
  );
}
