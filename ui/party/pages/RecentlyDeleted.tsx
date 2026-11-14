import { useCallback, useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import { deleteForever, listDeleted, restoreDeleted, type DeletedItem, type DeletedKind } from "../api/party";
import { purgeIn } from "./AdminTrash";
import "./party.css";

/** What a delete's confirm says about getting it back (0.3.6). */
export const KEPT_NOTE = "It stays in Account › Recently deleted for 30 days, where you can restore it.";

const DAY_MS = 24 * 60 * 60 * 1000;
const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

const KIND_WORD: Record<DeletedKind, string> = { party: "Party", encounter: "Encounter", character: "Character" };

/** A row's name: its own, else the kind ("Character"). */
export function deletedLabel(item: DeletedItem): string {
  return item.name.trim() || KIND_WORD[item.kind];
}

/** What a restore brings back, for its confirm. */
export function restoreNote(item: DeletedItem): string {
  switch (item.kind) {
    case "party": return "Its members, invites and encounters come back with it. A combat that was running comes back ended.";
    case "encounter": return item.detail ? `It goes back into ${item.detail}.` : "It goes back into its party.";
    case "character": return "It comes back in your characters, and on your devices at the next sync.";
  }
}

/**
 * Account › Recently deleted: the caller's own deleted parties, encounters and characters of the last 30 days, each
 * with Restore and "Delete forever" (an app confirm). The server removes them for good after that.
 */
export function RecentlyDeletedCard() {
  const toast = useToast();
  const confirm = useConfirm();
  const [items, setItems] = useState<DeletedItem[] | null>(null);
  const [days, setDays] = useState(30);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const load = useCallback(() => {
    listDeleted().then((t) => { setItems(t.items); setDays(t.retentionDays); setError(""); }, (e) => setError(message(e)));
  }, []);
  useEffect(load, [load]);

  const restore = async (item: DeletedItem) => {
    const answer = await confirm({
      title: `Restore ${deletedLabel(item)}?`,
      body: restoreNote(item),
      confirmLabel: "Restore",
    });
    if (answer !== "confirm") return;
    setBusy(`${item.kind}/${item.id}`);
    try {
      await restoreDeleted(item.kind, item.id);
      toast.push(`${deletedLabel(item)} is back.`, "info");
    } catch (e) { toast.push(message(e), "error"); }
    finally { setBusy(""); load(); }
  };

  const forget = async (item: DeletedItem) => {
    const answer = await confirm({
      title: `Delete ${deletedLabel(item)} forever?`,
      body: item.kind === "party"
        ? "The party and its deleted encounters are removed for good. This can't be undone."
        : "It is removed for good. This can't be undone.",
      confirmLabel: "Delete forever",
      danger: true,
    });
    if (answer !== "confirm") return;
    setBusy(`${item.kind}/${item.id}`);
    try {
      await deleteForever(item.kind, item.id);
      toast.push(`${deletedLabel(item)} was deleted for good.`, "info");
    } catch (e) { toast.push(message(e), "error"); }
    finally { setBusy(""); load(); }
  };

  const now = Date.now();
  return (
    <Card title="Recently deleted" className="bp-deleted">
      <p className="muted">Parties, encounters and characters you deleted in the last {days} days. After that they are removed for good.</p>
      {error && <Alert tone="danger">{error}</Alert>}
      {items?.length === 0 && <EmptyState title="Nothing deleted lately" />}
      <ul className="party-list">
        {items?.map((item) => {
          const key = `${item.kind}/${item.id}`;
          return (
            <li key={key} className="party-line bp-deleted-row" data-kind={item.kind}>
              <span className="bp-deleted-name">
                <strong>{deletedLabel(item)}</strong>
                <span className="muted"> · {KIND_WORD[item.kind]}{item.detail ? ` in ${item.detail}` : ""} · deleted {new Date(item.deletedAtMs).toLocaleDateString()}</span>
              </span>
              <span className="spacer" />
              {/* Never more than the retention: the server's clock may run a little ahead of this device's. */}
              <Chip tone="info">Removed {purgeIn(Math.min(item.purgeAtMs, now + days * DAY_MS), now)}</Chip>
              <span className="bp-deleted-actions">
                <Button small disabled={busy === key} onClick={() => void restore(item)}>Restore</Button>
                <Button small variant="danger" disabled={busy === key} onClick={() => void forget(item)}>Delete forever</Button>
              </span>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}
