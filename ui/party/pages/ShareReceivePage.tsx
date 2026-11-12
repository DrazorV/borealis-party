import { type FormEvent, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { openBundleFile } from "../../src/pages/transfer/openBundle";
import { useApi } from "../../src/state/ApiContext";
import { errorMessage, useAsync } from "../../src/state/useAsync";
import {
  acceptShare, addShareLinks, declineShare, listShares, onShareOutcome, type AddedLinks, type ReceivedShare, type Redeemed, redeemShare, reopenShare,
} from "../api/overrides/shares";
import { contents, SHARE_ROUTE, sizeText } from "./SharePage";
import "./party.css";

/** Where the Import screen sends a code (and where a share link opens). */
export const receiveRoute = (code?: string) => (code ? `/share/receive/${encodeURIComponent(code)}` : "/share/receive");

type Stage = "open" | "accepted" | "closed" | "declined";

export const BLOCK_TITLE = "Decline and block?";
export const blockBody = (from: string) => `Nothing from this share comes in, and share codes from ${from} stop working for you. You can unblock them later under Share with a code.`;

/** The source links of a received share: added through the server, then downloaded. */
function Links({ share, auto }: { share: ReceivedShare; auto: boolean }) {
  const api = useApi();
  const [state, setState] = useState<"idle" | "working" | "done">("idle");
  const [added, setAdded] = useState<AddedLinks | null>(null);
  const [message, setMessage] = useState("");
  const started = useRef(false);
  const add = async () => {
    setState("working");
    setMessage("");
    try {
      const out = await addShareLinks(share.id);
      setAdded(out);
      if (out.indexes.some((i) => i.added && !i.already)) {
        setMessage("Downloading the content from the added indexes…");
        await api.updateIndexes();
        setMessage("The content from the added indexes is downloaded.");
      }
      if (share.characters + share.packs === 0) await acceptShare(share.id).catch(() => undefined);
    } catch (e) {
      setMessage(errorMessage(e));
    }
    setState("done");
  };
  useEffect(() => {
    if (auto && !started.current) { started.current = true; void add(); }
  }, [auto]);
  const indexes = share.links.filter((l) => l.kind === "index");
  // As the server checked them once added; until then (or if it did not say), as the share lists them.
  const fivetools = share.links.filter((l) => l.kind === "5etools")
    .map((l) => added?.fivetools.find((f) => f.url === l.url) ?? { ...l, allowed: true, message: undefined });
  return (
    <div className="party-share-links">
      <h3>Sources</h3>
      <p className="muted small">These packs travel as the address they came from; the server downloads them for you.</p>
      <ul className="party-share-list" aria-label="Source links">
        {indexes.map((l) => {
          const done = added?.indexes.find((i) => i.url === l.url);
          return (
            <li key={l.url} className="party-share-row">
              <div className="grow">
                <strong>{l.packs.join(", ")}</strong>
                <p className="muted small party-share-url">Index: {l.url}</p>
                {done && <p className="small" role="status">{done.added ? (done.already ? "You follow this index already." : "Added.") : done.message ?? "Not added."}</p>}
              </div>
            </li>
          );
        })}
        {fivetools.map((l) => (
          <li key={l.url} className="party-share-row">
            <div className="grow">
              <strong>{l.packs.join(", ")}</strong>
              <p className="muted small party-share-url">5etools data: {l.url}</p>
              <p className="small">
                {l.allowed ? <>Import it on the <Link to="/import-files">Import</Link> screen: paste this address under Paste a link, then choose the books.</> : l.message}
              </p>
            </div>
          </li>
        ))}
      </ul>
      {indexes.length > 0 && state !== "done" && (
        <Button variant="primary" icon="download" disabled={state === "working"} onClick={() => void add()}>{state === "working" ? "Adding…" : "Add these sources"}</Button>
      )}
      {message && <p className="small" role="status">{message}</p>}
    </div>
  );
}

/**
 * Enter a share code (plan E4): the server stages the shared bundle in this account's own folder and upstream's Open
 * bundle window previews it, with the 0.1.4 safe receive (nothing you have is replaced). Packs that travel as links
 * are added after. Below: shares redeemed and not yet accepted or declined.
 */
export function ShareReceivePage() {
  const { code: fromLink } = useParams();
  const confirm = useConfirm();
  const [code, setCode] = useState(fromLink ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [current, setCurrent] = useState<Redeemed | null>(null);
  const [stage, setStage] = useState<Stage>("open");
  const waiting = useAsync(() => listShares().then((l) => l.received), []);
  const reload = waiting.reload;
  const currentId = useRef<string | null>(null);

  useEffect(() => onShareOutcome((id, outcome) => {
    if (currentId.current === id) setStage(outcome === "accepted" ? "accepted" : "closed");
    reload();
  }), [reload]);

  const show = (out: Redeemed) => {
    currentId.current = out.share.id;
    setCurrent(out);
    setStage(out.bundle ? "open" : "accepted");
    if (out.bundle) openBundleFile(out.bundle);
    waiting.reload();
  };
  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try { await work(); } catch (e) { setError(errorMessage(e)); }
    setBusy(false);
  };
  const redeem = (e?: FormEvent) => {
    e?.preventDefault();
    if (!code.trim() || busy) return;
    void run(async () => show(await redeemShare(code.trim())));
  };
  // A share link opens this page with the code: redeem it once, as if typed.
  const auto = useRef(false);
  useEffect(() => {
    if (fromLink && !auto.current) { auto.current = true; redeem(); }
  }, [fromLink]);

  const decline = (share: ReceivedShare, block: boolean) => void run(async () => {
    if (block && await confirm({ title: BLOCK_TITLE, body: blockBody(share.from), confirmLabel: "Decline and block", danger: true }) !== "confirm") return;
    await declineShare(share.id, block);
    if (current?.share.id === share.id) setStage("declined");
    waiting.reload();
  });
  const reopen = (share: ReceivedShare) => void run(async () => show(await reopenShare(share.id)));

  const others = (waiting.data ?? []).filter((s) => s.id !== current?.share.id || stage === "closed");
  return (
    <div className="party party-share">
      <Card title="Enter a share code">
        <p className="muted">Another player made a code with Share with a code. Enter it to see what it holds before anything comes in.</p>
        <form className="party-import-link" onSubmit={redeem}>
          <input className="input party-share-input" autoCapitalize="characters" autoCorrect="off" autoComplete="off" spellCheck={false}
            aria-label="Share code" placeholder="ABCD-EFGH-JKMN-PQRS" value={code} onChange={(e) => setCode(e.target.value)} />
          <Button type="submit" variant="primary" icon="download" disabled={busy || !code.trim()}>{busy ? "Opening…" : "Open"}</Button>
        </form>
        <p className="muted small"><Link to={SHARE_ROUTE}>Share your own with a code</Link></p>
      </Card>
      {error && <Alert tone="danger">{error}</Alert>}
      {current && (
        <Card title={`From ${current.share.from}`}>
          <p>{contents(current.share)}{current.share.bytes > 0 ? `, ${sizeText(current.share.bytes)}` : ""}.</p>
          {stage === "open" && <p className="muted" role="status">Choose in the Open bundle window what comes in.</p>}
          {stage === "accepted" && current.bundle && <Alert tone="success">Accepted. Anything you already had with different content was kept as yours.</Alert>}
          {stage === "declined" && <Alert tone="info">Declined: nothing came in.</Alert>}
          {stage === "closed" && (
            <div className="party-import-actions">
              <p className="muted grow">Closed without a choice. It waits below until you accept or decline it.</p>
            </div>
          )}
          {stage === "accepted" && current.share.links.length > 0 && <Links share={current.share} auto={!current.bundle} />}
          {stage !== "declined" && stage !== "accepted" && (
            <div className="party-import-actions">
              <Button disabled={busy} onClick={() => decline(current.share, false)}>Decline</Button>
              <Button disabled={busy} onClick={() => decline(current.share, true)}>Decline and block</Button>
            </div>
          )}
        </Card>
      )}
      {others.length > 0 && (
        <Card title="Waiting for you">
          <ul className="party-share-list" aria-label="Shares waiting for you">
            {others.map((s) => (
              <li key={s.id} className="party-share-row">
                <div className="grow">
                  <strong>{s.from}</strong>
                  <p className="muted small">{contents(s)}</p>
                </div>
                <Button small variant="primary" disabled={busy} onClick={() => reopen(s)}>Open</Button>
                <Button small disabled={busy} onClick={() => decline(s, false)}>Decline</Button>
              </li>
            ))}
          </ul>
        </Card>
      )}
    </div>
  );
}
