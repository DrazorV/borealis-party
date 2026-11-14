import { type ReactNode, useEffect, useState } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router";
import { Alert, EmptyState } from "../../src/components/Card";
import { Tabs } from "../../src/components/Tabs";
import { errorMessage } from "../../src/state/useAsync";
import { useDmRole } from "../api/dmRole";
import { dungeonMaster, listParties, type PartySummary } from "../api/overrides/parties";
import { AssistantButton } from "../assistant/AssistantButton";
import { EncounterBuilder } from "./EncounterBuilder";
import { useWide } from "./EncounterToolPanel";
import { DmEncounterList, encountersPath, NewEncounterDialog, usePartyEncounters } from "./EncountersPage";
import "./party.css";
import "./encounters.css";

/** The party last opened on the Encounters tab, per device. */
const LAST_KEY = "party.encountersParty";
const lastParty = () => { try { return localStorage.getItem(LAST_KEY); } catch { return null; } };
const keepParty = (id: string) => { try { localStorage.setItem(LAST_KEY, id); } catch { /* not remembered */ } };

/**
 * The Encounters main tab (0.3.1) is for accounts with the Dungeon Master switch on. Anyone else (and anyone who
 * types the address) goes to the start page; the server refuses every encounter call but the party's DM's anyway.
 * Until this device knows the answer, it asks the server once.
 */
export function DmOnly({ children }: { children: ReactNode }) {
  const dm = useDmRole();
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (dm !== null) return;
    let live = true;
    dungeonMaster().catch(() => { if (live) setFailed(true); });
    return () => { live = false; };
  }, [dm]);
  if (dm === false || failed) return <Navigate to="/" replace />;
  if (dm === null) return <div className="party enc-page"><p className="muted">Loading…</p></div>;
  return <>{children}</>;
}

/**
 * The Encounters tab: the DM's session prep. Its parties are the sub-tabs in the shell's band (Aurora's look); a big
 * screen shows the party's encounters on the left and the open one beside them, using the whole width. A phone shows
 * the list, then the builder on its own.
 */
export function EncounterWorkspace() {
  const { party: partyId = "", encounter = "" } = useParams();
  const navigate = useNavigate();
  const wide = useWide();
  const [parties, setParties] = useState<PartySummary[] | null | undefined>(undefined);
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    listParties().then((l) => { if (live) setParties(l ? l.parties.filter((p) => p.role === "dm") : null); },
      (e) => { if (live) setError(errorMessage(e)); });
    return () => { live = false; };
  }, []);
  useEffect(() => { if (partyId) keepParty(partyId); }, [partyId]);

  if (error) return <div className="party enc-page"><Alert tone="danger">{error}</Alert></div>;
  if (parties === undefined) return <div className="party enc-page"><p className="muted">Loading your parties…</p></div>;
  if (parties === null || parties.length === 0) {
    return (
      <div className="party enc-page">
        <EmptyState title="No party to prepare for yet">
          Encounters belong to a party you run. <Link to="/parties">Make a party</Link>, then build its encounters here.
        </EmptyState>
      </div>
    );
  }
  if (!partyId) {
    const remembered = lastParty();
    const pick = parties.find((p) => p.id === remembered) ?? parties[0]!;
    return <Navigate to={encountersPath(pick.id)} replace />;
  }
  const party = parties.find((p) => p.id === partyId);
  const strip = (
    <Tabs inBand label="Parties" idPrefix="enc-party" active={partyId}
      tabs={parties.map((p) => ({ id: p.id, label: p.name }))}
      onChange={(id) => void navigate(encountersPath(id))} />
  );
  if (!party) {
    return (
      <div className="party enc-page">
        {strip}
        <Alert tone="info">This party isn't there any more, or you don't run it.</Alert>
      </div>
    );
  }
  if (wide) {
    return (
      <div className="enc-ws">
        {strip}
        <nav className="enc-ws-list" aria-label={`${party.name}: encounters`}>
          <PartyList partyId={partyId} current={encounter} />
        </nav>
        <div className="enc-ws-main">
          {encounter
            ? <EncounterBuilder key={encounter} />
            : (
              <div className="enc-ws-empty">
                <EmptyState title="Pick an encounter">Open one from the list, or make a new one for {party.name}.</EmptyState>
              </div>
            )}
        </div>
      </div>
    );
  }
  return (
    <>
      {strip}
      {encounter ? <EncounterBuilder key={encounter} /> : (
        <div className="party enc-page">
          <PartyList partyId={partyId} />
        </div>
      )}
    </>
  );
}

function PartyList({ partyId, current }: { partyId: string; current?: string }) {
  const { list, error, reload } = usePartyEncounters(partyId);
  const [creating, setCreating] = useState(false);
  // One just made, or just deleted, shows at once (the server's event follows).
  const known = list?.role === "dm" && Boolean(current) && list.encounters.some((e) => e.id === current);
  useEffect(() => { if (list && !known) void reload(); }, [current]);
  return (
    <>
      {error && <Alert tone="danger">{error}</Alert>}
      {list === undefined && !error && <p className="muted">Loading encounters…</p>}
      {list === null && <Alert tone="info">This party isn't there any more, or it isn't yours.</Alert>}
      {list?.role === "dm" && <DmEncounterList partyId={partyId} list={list} current={current} onNew={() => setCreating(true)}
        tools={<AssistantButton purpose="encounter" context={{ partyId }} onChanged={() => void reload()} />} />}
      {creating && <NewEncounterDialog partyId={partyId} onClose={() => setCreating(false)} />}
    </>
  );
}
