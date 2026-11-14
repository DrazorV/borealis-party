import { type FormEvent, useState } from "react";
import { Link } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Check, SelectField, TextField } from "../../src/components/Fields";
import { useApi } from "../../src/state/ApiContext";
import { errorMessage, useAsync } from "../../src/state/useAsync";
import { cancelShare, createShare, listShares, type MadeShare, type NewShare, type SentShare, unblockSender } from "../api/overrides/shares";
import "./party.css";

/** The route of this page; the receive page is {@link RECEIVE_ROUTE}. */
export const SHARE_ROUTE = "/share";
export const RECEIVE_ROUTE = "/share/receive";

const PACK_OPTIONS = [
  { value: "required", label: "Content packs the characters need" },
  { value: "all", label: "All my content packs" },
  { value: "none", label: "No content packs" },
] as const;

export const LICENCE_TITLE = "Send the files of licensed content?";
export const LICENCE_BODY =
  "Packs downloaded from an index or converted from 5etools data normally travel as their address, and the other player downloads them again. Sending their files instead hands over a copy of the content. Do this only when the other player may use that content too (they own the books it came from, or its licence allows sharing).";

export const sizeText = (n: number): string => (n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / (1024 * 1024)).toFixed(1)} MB`);
const plural = (n: number, one: string) => (n === 1 ? `1 ${one}` : `${n} ${one}s`);
const when = (seconds: number) => new Date(seconds * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

/** "2 characters, 1 content pack and 1 link", for a share's contents. */
export function contents(s: { characters: number; packs: number; links: unknown[] }): string {
  const parts = [
    s.characters > 0 ? plural(s.characters, "character") : "",
    s.packs > 0 ? plural(s.packs, "content pack") : "",
    s.links.length > 0 ? plural(s.links.length, "source link") : "",
  ].filter(Boolean);
  if (parts.length === 0) return "Nothing";
  return parts.length === 1 ? parts[0]! : `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
}

const STATUS: Record<SentShare["status"], string> = {
  pending: "Waiting to be used",
  claimed: "Opened",
  done: "Accepted",
  declined: "Declined",
  cancelled: "Cancelled",
  expired: "Expired",
};

/** Copy text; true when the clipboard took it (an http:// page or an old web view may refuse). */
async function copy(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}

/** The code just made: large, with Copy (and the phone's share sheet where there is one). */
function MadeCode({ made, onDone }: { made: MadeShare; onDone: () => void }) {
  const [copied, setCopied] = useState("");
  const text = made.link ? `Borealis Connect share code: ${made.code}\n${made.link}` : `Borealis Connect share code: ${made.code}`;
  const share = typeof navigator.share === "function" ? () => navigator.share({ text }).catch(() => undefined) : null;
  return (
    <Card title="Your share code">
      <p className="party-share-code" aria-label="Share code" data-testid="share-code">{made.code}</p>
      <p className="muted">
        {contents(made.share)}, {sizeText(Math.max(made.share.bytes, 1))}. Works once, until {when(made.share.expiresAt)}
        {made.share.forUsername ? `, only for ${made.share.forUsername}` : ""}. The other player enters it under Import, Enter a share code{made.link ? ", or opens the link" : ""}.
      </p>
      {made.link && (
        <input className="input party-share-link" readOnly value={made.link} aria-label="Share link" onFocus={(e) => e.currentTarget.select()} />
      )}
      {made.leftOut.length > 0 && (
        <Alert tone="warning">Left out, as they can only travel as files: {made.leftOut.join(", ")}. Turn on "Include the files" to send them.</Alert>
      )}
      <div className="party-import-actions">
        <Button variant="primary" icon="copy" onClick={() => void copy(made.code).then((ok) => setCopied(ok ? "Code copied." : "Select the code and copy it."))}>Copy code</Button>
        {made.link && <Button icon="link" onClick={() => void copy(made.link!).then((ok) => setCopied(ok ? "Link copied." : "Select the link and copy it."))}>Copy link</Button>}
        {share && <Button icon="send" onClick={() => void share()}>Send…</Button>}
        <Button onClick={onDone}>Done</Button>
      </div>
      {copied && <p className="muted small" role="status">{copied}</p>}
    </Card>
  );
}

/**
 * Share with a code (plan E4): pick characters and content packs, and the server keeps a bundle of them for another
 * player, who types the code (or opens its link) on any device. Nothing is saved on this device, so it works on
 * Android too. Below: this account's codes, and the senders it blocked.
 */
export function SharePage() {
  const api = useApi();
  const confirm = useConfirm();
  const characters = useAsync(() => api.listCharacters(), [api]);
  const shares = useAsync(() => listShares(), []);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [packs, setPacks] = useState<NewShare["packs"]>("required");
  const [includeFiles, setIncludeFiles] = useState(false);
  const [forUsername, setForUsername] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [made, setMade] = useState<MadeShare | null>(null);

  const toggle = (id: string) => setChosen((set) => {
    const next = new Set(set);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });
  const askFiles = async (on: boolean) => {
    if (!on) { setIncludeFiles(false); return; }
    const answer = await confirm({ title: LICENCE_TITLE, body: LICENCE_BODY, confirmLabel: "Include the files" });
    setIncludeFiles(answer === "confirm");
  };
  const nothing = chosen.size === 0 && packs === "none";
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy || nothing) return;
    setBusy(true);
    setError("");
    try {
      const out = await createShare({ characterIds: [...chosen], packs, includeFiles, forUsername: forUsername.trim() || undefined });
      setMade(out);
      shares.reload();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  const act = async (work: () => Promise<unknown>) => {
    setError("");
    try { await work(); } catch (err) { setError(errorMessage(err)); }
    shares.reload();
  };

  const list = shares.data;
  const open = list?.sent.filter((s) => s.status === "pending" || s.status === "claimed") ?? [];
  const ended = list?.sent.filter((s) => s.status !== "pending" && s.status !== "claimed") ?? [];
  return (
    <div className="party party-share">
      {made ? <MadeCode made={made} onDone={() => setMade(null)} /> : (
        <Card title="Share with a code">
          <p className="muted">
            Another player gets these with a short code, on any device: no file to send. Packs from an index or from
            5etools data travel as their address, and the other player downloads them again.
            {" "}<Link to={RECEIVE_ROUTE}>Got a code? Enter it here.</Link>
          </p>
          <form className="party-share-form" onSubmit={(e) => void submit(e)}>
            <fieldset className="party-share-characters" disabled={busy}>
              <legend>Characters</legend>
              {characters.loading && <p className="muted small">Loading…</p>}
              {characters.data?.length === 0 && <p className="muted small">No characters yet. You can still share content packs.</p>}
              {characters.data?.map((c) => (
                <Check key={c.id} label={c.name} description={c.build || undefined} checked={chosen.has(c.id)} onChange={() => toggle(c.id)} />
              ))}
            </fieldset>
            <SelectField label="Content packs" options={PACK_OPTIONS} value={packs} onChange={(v) => setPacks(v as NewShare["packs"])} disabled={busy} />
            <Check label="Include the files" description="Send downloaded and 5etools packs as files instead of links." checked={includeFiles}
              disabled={busy || packs === "none"} onChange={(e) => void askFiles(e.currentTarget.checked)} />
            <TextField label="Only for (optional)" hint="A username. Anyone signed in to this server can use the code otherwise."
              autoCapitalize="none" autoCorrect="off" spellCheck={false} value={forUsername} onChange={(e) => setForUsername(e.target.value)} disabled={busy} />
            <div className="party-import-actions">
              <Button type="submit" variant="primary" icon="link" disabled={busy || nothing}>{busy ? "Making the code…" : "Make a share code"}</Button>
            </div>
          </form>
        </Card>
      )}
      {error && <Alert tone="danger">{error}</Alert>}
      <Card title="Your share codes">
        {shares.error && <Alert tone="danger">{shares.error}</Alert>}
        {list && open.length === 0 && ended.length === 0 && <p className="muted">None yet.</p>}
        {(open.length > 0 || ended.length > 0) && (
          <ul className="party-share-list" aria-label="Your share codes">
            {[...open, ...ended].map((s) => (
              <li key={s.id} className="party-share-row">
                <div className="grow">
                  <strong>{contents(s)}</strong>
                  <span className="muted small"> {STATUS[s.status]}{s.claimedBy ? ` by ${s.claimedBy}` : ""}{s.forUsername ? ` · for ${s.forUsername}` : ""}</span>
                  <p className="muted small">{s.status === "pending" || s.status === "claimed" ? `Until ${when(s.expiresAt)}` : `Made ${when(s.createdAt)}`}</p>
                </div>
                {s.status === "pending" && <Button small onClick={() => void act(() => cancelShare(s.id))}>Cancel</Button>}
              </li>
            ))}
          </ul>
        )}
        {list && (
          <p className="muted small">
            Codes work once, for {list.limits.days} days. At most {list.limits.openPerSender} open at a time and {sizeText(list.limits.maxBytes)} each.
          </p>
        )}
      </Card>
      {list && list.blocked.length > 0 && (
        <Card title="Blocked">
          <p className="muted">Share codes from these players don't work for you.</p>
          <ul className="party-share-list" aria-label="Blocked players">
            {list.blocked.map((b) => (
              <li key={b.id} className="party-share-row">
                <strong className="grow">{b.username}</strong>
                <Button small onClick={() => void act(() => unblockSender(b.id))}>Unblock</Button>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </div>
  );
}
