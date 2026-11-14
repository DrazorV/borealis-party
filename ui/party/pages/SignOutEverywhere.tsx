import { useState } from "react";
import { Button } from "../../src/components/Button";
import { Card } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { useToast } from "../../src/components/Toast";
import { type SignedOutEverywhere, signOutEverywhereElse } from "../api/party";

const message = (e: unknown): string => (e as { message?: string }).message ?? "Something went wrong.";
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The toast after "Sign out everywhere": what ended, in counts. */
export function signedOutSummary(r: SignedOutEverywhere): string {
  if (r.sessions + r.apiTokens + r.apps === 0) return "Nothing else was signed in. This device stays signed in.";
  const parts = [
    plural(r.sessions, "other sign-in", "other sign-ins"),
    plural(r.apps, "AI app", "AI apps"),
    plural(r.apiTokens, "API token", "API tokens"),
  ];
  return `Signed out: ${parts.join(", ")}. This device stays signed in.`;
}

/**
 * Account › Sign out everywhere: ends the account's other sign-ins, its connected AI apps (ChatGPT, Claude) and its
 * API tokens, for a lost phone or a token that leaked. Changing the password does the same.
 */
export function SignOutEverywhereCard() {
  const toast = useToast();
  const confirm = useConfirm();
  const [busy, setBusy] = useState(false);
  const run = async () => {
    const answer = await confirm({
      title: "Sign out everywhere",
      body: "Sign out every other device, disconnect every AI app and revoke every API token? This device stays signed in. AI apps have to connect again.",
      confirmLabel: "Sign out everywhere",
      danger: true,
    });
    if (answer !== "confirm") return;
    setBusy(true);
    try { toast.push(signedOutSummary(await signOutEverywhereElse()), "info"); }
    catch (e) { toast.push(message(e), "error"); }
    finally { setBusy(false); }
  };
  return (
    <Card title="Sign out everywhere">
      <p className="muted">Ends your other sign-ins, your connected AI apps and your API tokens. Changing your password does this too.</p>
      <div className="party-inline">
        <Button variant="danger" disabled={busy} onClick={() => void run()}>Sign out everywhere</Button>
      </div>
    </Card>
  );
}
