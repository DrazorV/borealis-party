import { useEffect, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { adminDefaultSource, saveAdminDefaultSource, type DefaultSourceSettings } from "../api/importSource";
import { checkFiveToolsSource } from "../import/sniff";
import "./importSource.css";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";

function where(s: DefaultSourceSettings): string {
  const file = s.configFile ?? "nothing";
  if (s.source === "admin") return `Saved here, which wins over config.toml (config.toml says ${file}).`;
  if (s.source === "config") return "From config.toml or BOREALIS__IMPORT__DEFAULT_5ETOOLS_URL.";
  return "None: the Import screen offers only each account's own address.";
}

/**
 * Admin › Settings › Default 5etools source: an address the Import screen offers every account in one tap. The party
 * ships none; the server checks the address as it would any download (import_source.rs).
 */
export function DefaultFiveToolsSource() {
  const [s, setS] = useState<DefaultSourceSettings | null>(null);
  const [entry, setEntry] = useState("");
  const [error, setError] = useState("");
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => { adminDefaultSource().then((v) => { setS(v); setEntry(v.url ?? ""); }, (e) => setError(message(e))); }, []);

  const save = async (url: string | null) => {
    setBusy(true);
    setFieldError(null);
    setSaved(false);
    try {
      const next = await saveAdminDefaultSource(url);
      setS(next);
      setEntry(next.url ?? "");
      setSaved(true);
    } catch (e) {
      setFieldError(message(e));
    } finally { setBusy(false); }
  };
  const submit = () => {
    const checked = checkFiveToolsSource(entry);
    if (checked.error !== undefined) { setFieldError(checked.error); setSaved(false); return; }
    void save(checked.url);
  };

  if (error) return <Alert tone="danger">{error}</Alert>;
  if (!s) return null;
  return (
    <Card title="Default 5etools source">
      <form className="party-form" noValidate onSubmit={(e) => { e.preventDefault(); submit(); }}>
        <p className="muted">
          An address of 5etools-format data the Import screen offers every account in one tap, for example a GitHub
          repository your group is entitled to use. Borealis Party comes with none. Accounts can always import from an
          address of their own.
        </p>
        <p className="muted small">{where(s)}</p>
        <label className="field">
          <span className="field-label">Address</span>
          <input className="input" type="url" inputMode="url" autoCapitalize="none" autoCorrect="off" spellCheck={false}
            aria-label="Default 5etools source address" aria-invalid={fieldError ? true : undefined}
            placeholder="https://github.com/owner/repo" value={entry}
            onChange={(e) => { setEntry(e.target.value); setFieldError(null); setSaved(false); }} />
        </label>
        {fieldError && <p className="party-import-field-error" role="alert">{fieldError}</p>}
        {saved && <Alert tone="success" role="status">{s.url ? "Saved; the Import screen offers it now." : "Saved; no default is offered now."}</Alert>}
        <div className="party-inline">
          <Button type="submit" variant="primary" disabled={busy || !entry.trim() || entry.trim() === s.url}>Save</Button>
          {s.url && <Button disabled={busy} onClick={() => void save(s.configFile === null ? null : "")}>Offer none</Button>}
          {s.source === "admin" && s.configFile !== null && <Button disabled={busy} onClick={() => void save(null)}>Use config.toml</Button>}
        </div>
      </form>
    </Card>
  );
}
