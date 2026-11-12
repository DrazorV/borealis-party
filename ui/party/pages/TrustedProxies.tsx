import { useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip } from "../../src/components/Card";
import { TextArea } from "../../src/components/Fields";
import { adminProxies, saveAdminProxies, type ProxySettings } from "../api/party";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

const IPV4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;
const IPV6 = /^\[?[0-9a-f:.]*:[0-9a-f:.]*\]?$/i;

/**
 * What is wrong with one entry of the list, or null. The server checks again (`admin::checked_proxies`); this only
 * says so before saving.
 */
export function proxyProblem(entry: string): string | null {
  const [address = "", prefix, extra] = entry.split("/");
  const v4 = IPV4.exec(address);
  const v6 = !v4 && IPV6.test(address);
  if (extra !== undefined || (!v4 && !v6) || (v4 && v4.slice(1).some((n) => Number(n) > 255))) {
    return `${entry} is not an address or a range like 172.20.20.0/24.`;
  }
  if (prefix === undefined) return null;
  const max = v4 ? 32 : 128;
  const bits = /^\d{1,3}$/.test(prefix) ? Number(prefix) : NaN;
  if (!(bits <= max)) return `${entry}: the prefix length must be 0 to ${max}.`;
  if (bits === 0) return `${entry} would trust every address, so anyone could pretend to be anyone.`;
  const widest = v4 ? 8 : 32;
  if (bits < widest) return `${entry} is too wide a range to trust (/${widest} at most).`;
  return null;
}

/** The list as typed: one entry per line (commas and spaces also separate), blanks and repeats dropped. */
export function parseProxies(text: string): string[] {
  return [...new Set(text.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean))];
}

function where(p: ProxySettings): string {
  const file = p.configFile.length ? p.configFile.join(", ") : "nothing";
  if (p.source === "admin") return `Saved here, which wins over config.toml (config.toml lists ${file}).`;
  if (p.source === "config") return `From config.toml or BOREALIS__SERVER__TRUSTED_PROXIES: ${file}.`;
  return "No proxy is trusted (the default): every request counts as coming from the address that connects.";
}

function Diagnosis({ p }: { p: ProxySettings }) {
  const forwarded = p.headers.cfConnectingIp !== null || p.headers.xForwardedFor !== null;
  if (p.peerTrusted && p.clientIp !== p.peer) {
    return <Alert tone="success">Working: {p.peer} is trusted, and the server sees your own address, {p.clientIp}.</Alert>;
  }
  if (p.peerTrusted) {
    return <Alert tone="info">{p.peer} is trusted, but your request names no client through it, so the server sees {p.peer}.</Alert>;
  }
  if (forwarded) {
    return (
      <Alert tone="warning">
        Your request came through a proxy at {p.peer} that the server doesn't trust, so every visitor looks like {p.peer} and
        they share the sign-up and sign-in limits: anyone can lock everyone out. If {p.peer} is your reverse proxy, trust it.
      </Alert>
    );
  }
  return (
    <Alert tone="info">
      Your request carries no forwarding headers, so it seems to reach the server directly. Trust {p.peer} only if it is a
      proxy you run.
    </Alert>
  );
}

/** Admin › Settings › Trusted proxies: which reverse proxies may name the client of a request. */
export function TrustedProxies() {
  const [p, setP] = useState<ProxySettings | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const show = (next: ProxySettings) => { setP(next); setText(next.proxies.join("\n")); };
  useEffect(() => { adminProxies().then(show, (e) => setError(message(e))); }, []);

  const save = async (list: string[] | null) => {
    setBusy(true);
    setSaveError(null);
    try {
      const next = await saveAdminProxies(list);
      show(next);
      setSaved(next.clientIp);
    } catch (e) {
      setSaveError(message(e));
      setSaved(null);
    } finally { setBusy(false); }
  };

  if (error) return <Alert tone="danger">{error}</Alert>;
  if (!p) return null;
  const typed = parseProxies(text);
  const problem = typed.map(proxyProblem).find((m) => m !== null) ?? null;
  const changed = typed.join("\n") !== p.proxies.join("\n");

  return (
    <Card title="Trusted proxies">
      <div className="party-form">
        <p className="muted">
          A reverse proxy (Cloudflare, Nginx Proxy Manager) connects to the server for every visitor. Trust it, and the
          server reads the visitor's own address from the headers it adds, so each visitor gets their own sign-up and
          sign-in limits. Only trust proxies you run: a trusted address can claim to be anyone.
        </p>
        <dl className="party-facts">
          <div><dt>Requests arrive from</dt><dd>{p.peer} {p.peerTrusted && <Chip tone="success">Trusted</Chip>}</dd></div>
          <div><dt>CF-Connecting-IP</dt><dd>{p.headers.cfConnectingIp ?? <span className="muted">not on your request</span>}</dd></div>
          <div><dt>X-Forwarded-For</dt><dd>{p.headers.xForwardedFor ?? <span className="muted">not on your request</span>}</dd></div>
          <div><dt>Your address, as the server sees it</dt><dd>{p.clientIp}</dd></div>
        </dl>
        <Diagnosis p={p} />
        {saved !== null && <Alert tone="success" role="status">Saved; it applies now. The server sees your request as coming from {saved}.</Alert>}
        {!p.peerTrusted && (
          <div className="party-inline">
            <Button variant="primary" disabled={busy} onClick={() => void save([...p.proxies.filter((e) => e !== p.peer), p.peer])}>
              Trust this proxy ({p.peer})
            </Button>
          </div>
        )}
        <TextArea
          label="Trusted proxies"
          hint="Addresses or ranges, one per line, such as 172.20.20.100 or 172.18.0.0/16. Empty: trust no proxy."
          error={problem ?? saveError}
          rows={3}
          spellCheck={false}
          autoCapitalize="off"
          autoComplete="off"
          value={text}
          onChange={(e) => { setText(e.target.value); setSaveError(null); }}
        />
        <div className="party-inline">
          <Button disabled={busy || !changed || problem !== null} onClick={() => void save(typed)}>Save list</Button>
          {p.source === "admin" && <Button disabled={busy} onClick={() => void save(null)}>Use config.toml instead</Button>}
        </div>
        <p className="muted">In force: {where(p)}</p>
      </div>
    </Card>
  );
}
