import { useCallback, useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import { adminTrash, restoreFromTrash, type TrashItem } from "../api/party";
import "./party.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";
const DAY_MS = 24 * 60 * 60 * 1000;

/** "today", "in 1 day", "in 12 days": when the server's sweep removes the copy for good. */
export function purgeIn(purgeAtMs: number, nowMs: number): string {
  const days = Math.ceil((purgeAtMs - nowMs) / DAY_MS);
  if (days <= 0) return "today";
  return days === 1 ? "in 1 day" : `in ${days} days`;
}

/** What a row calls the character: its name, else its id. */
export function trashLabel(item: TrashItem): string {
  return item.name?.trim() || item.characterId;
}

/**
 * Admin › Trash: characters the accounts deleted in the last 30 days, with Restore. Names and dates only; the sheets
 * stay private. Deleted parties and encounters are in their DM's own Recently deleted (Account page); content is deleted
 * for good.
 */
export function Trash() {
  const toast = useToast();
  const confirm = useConfirm();
  const [items, setItems] = useState<TrashItem[] | null>(null);
  const [days, setDays] = useState(30);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const load = useCallback(() => {
    adminTrash().then((t) => { setItems(t.items); setDays(t.retentionDays); setError(""); }, (e) => setError(message(e)));
  }, []);
  useEffect(load, [load]);

  const restore = async (item: TrashItem) => {
    const answer = await confirm({
      title: "Restore character",
      body: <>Restore <strong>{trashLabel(item)}</strong> to <strong>{item.username}</strong>'s characters? It comes back on their devices at the next sync.</>,
      confirmLabel: "Restore",
    });
    if (answer !== "confirm") return;
    setBusy(`${item.accountId}/${item.characterId}`);
    try {
      await restoreFromTrash(item.accountId, item.characterId);
      toast.push(`${trashLabel(item)} restored for ${item.username}.`, "info");
    } catch (e) { toast.push(message(e), "error"); }
    finally { setBusy(""); load(); }
  };

  if (error) return <Alert tone="danger">{error}</Alert>;
  const now = Date.now();
  return (
    <Card title="Trash">
      <p className="muted">Characters deleted in the last {days} days. After that the server removes them for good. Each DM restores their own parties and encounters from Account › Recently deleted; content has no trash.</p>
      {items?.length === 0 && <EmptyState title="The trash is empty" />}
      <ul className="party-list">
        {items?.map((item) => (
          <li key={`${item.accountId}/${item.characterId}`} className="party-line">
            <span>
              <strong>{trashLabel(item)}</strong>
              <span className="muted"> · {item.username} · deleted {new Date(item.deletedAtMs).toLocaleString()}</span>
            </span>
            <span className="spacer" />
            <Chip tone="info">Removed {purgeIn(item.purgeAtMs, now)}</Chip>
            <Button small disabled={busy === `${item.accountId}/${item.characterId}`} onClick={() => void restore(item)}>Restore</Button>
          </li>
        ))}
      </ul>
    </Card>
  );
}
