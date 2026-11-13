import { useEffect, useState, useSyncExternalStore } from "react";
import type { ContentProgress } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { ProgressBar } from "../../src/components/Card";
import { useApi } from "../../src/state/ApiContext";
import { type Area, contentSync } from "./contentSync";
import "./content.css";

/**
 * The content changes still on their way to the server, above the list they change: "Removing…", "Adding…",
 * "Downloading…" with the progress the server sends, a short "Removed" when done, and the server's message when it
 * refused (until dismissed; the removed items are back in the list then).
 */
export function ContentActivity({ areas }: { areas: readonly Area[] }) {
  const api = useApi();
  const { activity } = useSyncExternalStore(contentSync.subscribe, contentSync.get);
  const rows = activity.filter((a) => areas.includes(a.area));
  const watching = rows.some((a) => a.progress && a.state === "working");
  const [progress, setProgress] = useState<ContentProgress | null>(null);
  useEffect(() => {
    if (!watching) { setProgress(null); return; }
    const off = [api.subscribe("content:progress", setProgress), api.subscribe("import5etools:progress", setProgress)];
    return () => off.forEach((f) => f());
  }, [api, watching]);
  if (rows.length === 0) return null;
  return (
    <ul className="party-content-activity" aria-label="Content changes">
      {rows.map((a) => (
        <li key={a.id} className={`party-activity party-activity-${a.state}`} role={a.state === "failed" ? "alert" : "status"}>
          <span className="party-activity-text">{a.state === "working" ? a.label : a.message ?? a.label}</span>
          {a.progress && a.state === "working" && progress && progress.phase !== "done" && progress.total > 0 && (
            <span className="party-activity-progress">
              <ProgressBar label={a.label} value={progress.done} max={Math.max(1, progress.total)} />
              {progress.message && <span className="small muted">{progress.message}</span>}
            </span>
          )}
          {a.state === "failed" && <Button small onClick={() => contentSync.dismiss(a.id)}>Dismiss</Button>}
        </li>
      ))}
    </ul>
  );
}
