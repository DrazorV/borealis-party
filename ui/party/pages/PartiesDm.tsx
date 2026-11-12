import { useEffect, useState } from "react";
import { Link } from "react-router";
import { Card } from "../../src/components/Card";
import { ToggleSwitch } from "../../src/components/controls/Controls";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import { dungeonMaster, setDungeonMaster } from "../api/overrides/parties";

/**
 * The Account page's "Dungeon Master tools" switch (plan D2): self-service, nobody approves it. On, the account can
 * create parties; its characters can join parties either way. The server refuses "off" while the account still runs
 * parties, and says so. Nothing shows when the server has no parties (switched off, or older).
 */
export function DungeonMasterCard() {
  const toast = useToast();
  const [on, setOn] = useState<boolean | null>(null);
  const [offered, setOffered] = useState(true);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    dungeonMaster().then((dm) => { if (live) setOn(dm); }, (e) => {
      if (!live) return;
      if ((e as { code?: string }).code === "not_found") setOffered(false);
      else setOn(false);
    });
    return () => { live = false; };
  }, []);
  const change = async (next: boolean) => {
    setBusy(true);
    try { setOn(await setDungeonMaster(next)); } catch (e) { toast.push(errorMessage(e), "error"); } finally { setBusy(false); }
  };
  if (!offered) return null;
  return (
    <Card title="Dungeon Master tools">
      <div className="party-line">
        <span className="grow muted">Run parties: invite players' characters and see their cards.</span>
        <ToggleSwitch label="Dungeon Master tools" checked={Boolean(on)} disabled={busy || on === null} onChange={(v) => void change(v)} />
      </div>
      {on && <p className="party-note"><Link to="/parties">Manage your parties</Link></p>}
    </Card>
  );
}
