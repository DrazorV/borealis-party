import { useState } from "react";
import { Link } from "react-router";
import { Card } from "../../src/components/Card";
import { ToggleSwitch } from "../../src/components/controls/Controls";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import { setDungeonMaster, type PartyUser } from "../api/overrides/parties";
import { loadSession, updateUser } from "../api/session";

/**
 * The Account page's "Dungeon Master tools" switch (plan D2): self-service, nobody approves it. On, the account can
 * create parties; its characters can join parties either way.
 */
export function DungeonMasterCard() {
  const toast = useToast();
  const [on, setOn] = useState(() => Boolean((loadSession()?.user as PartyUser | undefined)?.dm));
  const [busy, setBusy] = useState(false);
  const change = async (next: boolean) => {
    setBusy(true);
    try {
      const user = await setDungeonMaster(next);
      updateUser(user);
      setOn(Boolean(user.dm));
    } catch (e) { toast.push(errorMessage(e), "error"); }
    finally { setBusy(false); }
  };
  return (
    <Card title="Dungeon Master tools">
      <div className="party-line">
        <span className="grow muted">Run parties: invite players' characters and see their cards.</span>
        <ToggleSwitch label="Dungeon Master tools" checked={on} disabled={busy} onChange={(v) => void change(v)} />
      </div>
      {on && <p className="party-note"><Link to="/parties">Manage your parties</Link></p>}
    </Card>
  );
}
