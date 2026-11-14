import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import type { Api, AppSettings } from "../../src/api/contract";
import { Button, IconButton } from "../../src/components/Button";
import { Icon } from "../../src/components/Icon";
import { useApi } from "../../src/state/ApiContext";
import { useSettings } from "../../src/state/SettingsContext";
import { isOffline } from "../api/connectivity";
import { useDmRole } from "../api/dmRole";
import { listEncounters } from "../api/overrides/encounters";
import { listParties, onPartiesChanged } from "../api/overrides/parties";
import { loadSession } from "../api/session";
import { listApps, listTokens } from "../pages/AiApps";
import { buildSteps, DISMISS_KEY, type Facts, isDismissed, nextStep, progress, serverKeepsDismissal, showChecklist } from "./checklist";
import "./onboarding.css";

/** This device's answer, per account, for a server that doesn't keep the setting (and for the first paint). */
const LOCAL_KEY = "party.onboardingDismissed";

function readLocal(): boolean {
  const id = loadSession()?.user.id;
  if (!id) return false;
  try { return (JSON.parse(localStorage.getItem(LOCAL_KEY) ?? "{}") as Record<string, boolean>)[id] === true; } catch { return false; }
}
function writeLocal(): void {
  const id = loadSession()?.user.id;
  if (!id) return;
  try {
    const all = JSON.parse(localStorage.getItem(LOCAL_KEY) ?? "{}") as Record<string, boolean>;
    localStorage.setItem(LOCAL_KEY, JSON.stringify({ ...all, [id]: true }));
  } catch { /* a private window: the account setting still has it */ }
}

const yes = (p: Promise<boolean>): Promise<boolean | null> => p.catch(() => null);

/** What the account has, read in parallel; a failed read is "not known". */
export async function readFacts(api: Api): Promise<Facts> {
  const parties = listParties().catch(() => null);
  const [content, character, party, encounter, ai] = await Promise.all([
    yes(Promise.all([api.listPacks(), api.listIndexes().catch(() => [])])
      .then(([packs, indexes]) => packs.some((p) => p.kind !== "bundled") || indexes.length > 0)),
    yes(api.listCharacters().then((l) => l.length > 0)),
    parties.then((l) => (l ? l.parties.length > 0 : null)),
    parties.then(async (l) => {
      if (!l) return null;
      for (const p of l.parties.filter((x) => x.role === "dm")) {
        const list = await listEncounters(p.id).catch(() => null);
        if (list && list.encounters.length > 0) return true;
      }
      return false;
    }),
    // A server without AI apps answers 404: the step is left out.
    Promise.all([listTokens(), listApps()]).then(([t, a]) => t.length > 0 || a.length > 0, () => null),
  ]);
  return { content, character, party, encounter, ai };
}

/**
 * The first-run checklist on the Characters page (./checklist.ts): a card above the characters until every required
 * step is done or the account closes it. Nothing shows offline, or until the account's facts are read (no flash of
 * unticked steps for an account that has done them all).
 */
export function OnboardingCard() {
  const api = useApi();
  const nav = useNavigate();
  const { settings, loaded, update } = useSettings();
  const dm = useDmRole() === true;
  const [facts, setFacts] = useState<Facts | null>(null);
  const [local, setLocal] = useState(readLocal);
  const offline = isOffline();
  const dismissed = isDismissed(settings, local);

  useEffect(() => {
    if (offline || dismissed) return;
    let live = true;
    const read = () => void readFacts(api).then((f) => { if (live) setFacts(f); });
    read();
    // A party made or joined in another tab, or a DM switch change, ticks its step here too.
    const off = onPartiesChanged(read);
    return () => { live = false; off(); };
  }, [api, offline, dismissed, dm]);

  if (offline || !loaded || !facts) return null;
  const steps = buildSteps(facts, dm);
  if (!showChecklist(steps, dismissed)) return null;
  const { done, total } = progress(steps);
  const next = nextStep(steps);

  const close = async () => {
    writeLocal();
    setLocal(true);
    if (serverKeepsDismissal(settings)) await update({ [DISMISS_KEY]: true } as Partial<AppSettings>).catch(() => undefined);
  };

  return (
    <section className="card party-onboarding" aria-label="Get started">
      <div className="card-head">
        <h2>Get started</h2>
        <span className="muted small party-onboarding-count">{done} of {total} done</span>
        <span className="spacer" />
        <IconButton icon="x" label="Close the checklist" onClick={() => void close()} />
      </div>
      <p className="muted small">{dm ? "Set up your table: content, a party and a first encounter." : "Get to the table: content, a character and your DM's party."} Each step ticks itself when it is done.</p>
      <div className="party-onboarding-bar" role="progressbar" aria-label="Steps done" aria-valuemin={0} aria-valuemax={total} aria-valuenow={done}>
        <span style={{ width: `${total ? (done / total) * 100 : 0}%` }} />
      </div>
      <ol className="party-onboarding-steps">
        {steps.map((s) => (
          <li key={s.id} className={`party-onboarding-step${s.done ? " done" : ""}${s === next ? " next" : ""}`} data-step={s.id} data-done={s.done}>
            <span className="party-onboarding-mark" aria-hidden="true">{s.done ? <Icon name="check" /> : null}</span>
            <div className="party-onboarding-text">
              <strong>{s.title}{s.optional && <span className="muted small"> (optional)</span>}</strong>
              <span className="muted small">{s.done ? "Done." : s.detail}</span>
            </div>
            {!s.done && (
              <Button small variant={s === next ? "primary" : "default"} aria-label={`${s.action}: ${s.title}`} onClick={() => void nav(s.to)}>
                {s.action}
              </Button>
            )}
          </li>
        ))}
      </ol>
    </section>
  );
}
