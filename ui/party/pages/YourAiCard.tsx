import { type FormEvent, type MouseEvent, useEffect, useRef, useState } from "react";
import { Button } from "../../src/components/Button";
import { Alert, Card } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { CheckBox, ComboBox, ToggleSwitch } from "../../src/components/controls/Controls";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import { openExternal } from "../api/overrides/links";
import {
  aiStatus, removeAi, removeAiKey, saveAi, startOpenRouter, testConnection,
  type AiStatus, type Mode, type Profile, type ProviderKind, type SaveForm,
} from "../api/overrides/assistant";
import "../assistant/assistant.css";

/**
 * The Account page's "Your AI" (0.3.1 part C): the account's own AI provider for the assistant panel.
 *
 * - **Through the server:** an OpenAI-compatible address (OpenAI, OpenRouter, Groq, Together), Anthropic or Google
 *   Gemini, with the person's key. The server keeps the key encrypted and shows only its last four characters; it
 *   is never in an export, a bundle or a share.
 * - **On this device:** Ollama or LM Studio on this computer or the home network. This app talks to it; the server
 *   never does (its address guard refuses private addresses for every account), and no key is kept.
 *
 * - **Connect OpenRouter** (0.3.2): a browser sign-in instead of pasting a key. The browser (the system browser in the
 *   apps) goes to openrouter.ai; OpenRouter sends it back to the server, which saves the key like a pasted one. The
 *   card asks the server again until the key is there.
 *
 * When the server can't save keys, everyone sees a plain note; only an admin is told how to switch it on
 * (`keys.adminHelp`, which the server sends to admins only). Claude has no sign-in for other apps, so the card points
 * to "AI apps" (MCP) instead.
 *
 * Nothing is sent anywhere until the person agrees that their requests go to the provider they chose. Nothing shows
 * when the server doesn't offer the assistant (switched off, or older).
 */

/** How often, and how long, the card asks whether a browser sign-in finished. */
export const SIGN_IN_POLL_MS = 3000;
const SIGN_IN_WAIT_MS = 10 * 60 * 1000;

const OPENROUTER_BASE = "https://openrouter.ai/api/v1";

/** Scroll to the Account page's "AI apps" card (MCP), where claude.ai and Claude Desktop connect. */
function toAiApps(e: MouseEvent) {
  e.preventDefault();
  document.querySelector("section.ai-apps")?.scrollIntoView({ behavior: "smooth", block: "start" });
}

const PROVIDERS: { value: ProviderKind; label: string }[] = [
  { value: "openai", label: "OpenAI-compatible" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Google Gemini" },
];

/** Addresses people use most, to fill the field in one tap. */
const PRESETS: { label: string; mode: Mode; baseUrl: string; model: string }[] = [
  { label: "OpenAI", mode: "server", baseUrl: "https://api.openai.com/v1", model: "gpt-4.1-mini" },
  { label: "OpenRouter", mode: "server", baseUrl: "https://openrouter.ai/api/v1", model: "" },
  { label: "Groq", mode: "server", baseUrl: "https://api.groq.com/openai/v1", model: "" },
  { label: "Together", mode: "server", baseUrl: "https://api.together.xyz/v1", model: "" },
  { label: "Ollama on this device", mode: "device", baseUrl: "http://localhost:11434/v1", model: "llama3.1:8b" },
  { label: "LM Studio on this device", mode: "device", baseUrl: "http://localhost:1234/v1", model: "" },
];

const PROFILES: { value: "" | Profile; label: string }[] = [
  { value: "", label: "Guess from the model" },
  { value: "small", label: "Small (local or fast models)" },
  { value: "frontier", label: "Large (the biggest hosted models)" },
];

const PROVIDER_NAMES: Record<ProviderKind, string> = { openai: "the OpenAI-compatible provider", anthropic: "Anthropic", gemini: "Google" };

export function providerName(kind: ProviderKind, mode: Mode, baseUrl: string | null): string {
  if (mode === "device") return "the model on your device";
  if (kind !== "openai" || !baseUrl) return PROVIDER_NAMES[kind];
  try { return new URL(baseUrl).host; } catch { return PROVIDER_NAMES[kind]; }
}

export function YourAiCard() {
  const toast = useToast();
  const confirm = useConfirm();
  const [status, setStatus] = useState<AiStatus | null>(null);
  const [offered, setOffered] = useState(true);
  const [provider, setProvider] = useState<ProviderKind>("openai");
  const [mode, setMode] = useState<Mode>("server");
  const [baseUrl, setBaseUrl] = useState("https://api.openai.com/v1");
  const [model, setModel] = useState("gpt-4.1-mini");
  const [key, setKey] = useState("");
  const [consent, setConsent] = useState(false);
  const [autoApply, setAutoApply] = useState(false);
  const [profile, setProfile] = useState<"" | Profile>("");
  const [busy, setBusy] = useState(false);
  /** A browser sign-in in progress: the address it opened, and the consent time saved before it. */
  const [signingIn, setSigningIn] = useState<{ url: string; before: number | null } | null>(null);
  const live = useRef(true);
  useEffect(() => () => { live.current = false; }, []);

  const show = (s: AiStatus) => {
    setStatus(s);
    const p = s.provider;
    if (p) {
      setProvider(p.provider);
      setMode(p.mode);
      setBaseUrl(p.baseUrl ?? s.defaults[p.provider].baseUrl);
      setModel(p.model);
      setConsent(true);
      setAutoApply(p.autoApply);
      setProfile(p.profile);
    }
    setKey("");
  };

  useEffect(() => {
    let live = true;
    aiStatus().then((s) => { if (live) show(s); }, (e) => {
      if (live && (e as { code?: string }).code === "not_found") setOffered(false);
    });
    return () => { live = false; };
  }, []);

  // While a browser sign-in is open: ask the server again every few seconds and when this window is back in front.
  useEffect(() => {
    if (!signingIn) return;
    const until = Date.now() + SIGN_IN_WAIT_MS;
    const check = () => {
      if (Date.now() > until) { setSigningIn(null); return; }
      aiStatus().then((s) => {
        const p = s.provider;
        if (live.current && p?.hasKey && p.baseUrl === OPENROUTER_BASE && p.consentedAt !== signingIn.before) {
          show(s);
          setSigningIn(null);
          toast.push("OpenRouter is connected.", "info");
        }
      }, () => { /* try again on the next tick */ });
    };
    const timer = window.setInterval(check, SIGN_IN_POLL_MS);
    window.addEventListener("focus", check);
    return () => { window.clearInterval(timer); window.removeEventListener("focus", check); };
  }, [signingIn]);

  if (!offered) return null;
  const saved = status?.provider ?? null;
  const sameProvider = saved?.provider === provider && saved.mode === mode && mode === "server";
  const canStore = status?.keys.canStore ?? true;
  const offersOpenRouter = status?.signIn?.openrouter ?? false;

  const changeProvider = (next: ProviderKind) => {
    setProvider(next);
    if (next !== "openai") setMode("server");
    if (status) {
      setBaseUrl(status.defaults[next].baseUrl);
      setModel(saved?.provider === next ? saved.model : status.defaults[next].model);
    }
  };
  const preset = (label: string) => {
    const p = PRESETS.find((x) => x.label === label);
    if (!p) return;
    setProvider("openai");
    setMode(p.mode);
    setBaseUrl(p.baseUrl);
    if (p.model) setModel(p.model);
  };

  const act = async (work: () => Promise<AiStatus>, done: string) => {
    setBusy(true);
    try { show(await work()); toast.push(done, "info"); } catch (e) { toast.push(errorMessage(e), "error"); } finally { setBusy(false); }
  };

  const save = (e: FormEvent) => {
    e.preventDefault();
    const form: SaveForm = { provider, mode, model, consent, autoApply };
    if (profile) form.profile = profile;
    if (provider === "openai") form.baseUrl = baseUrl;
    if (mode === "server" && key.trim()) form.key = key.trim();
    void act(() => saveAi(form), "Your AI is saved.");
  };

  const test = async () => {
    if (!saved) return;
    setBusy(true);
    try {
      const r = await testConnection(saved);
      toast.push(r.ok ? `Connected: the model answered "${r.reply ?? ""}".` : (r.message ?? "The provider didn't answer."), r.ok ? "info" : "error");
    } catch (e) { toast.push(errorMessage(e), "error"); } finally { setBusy(false); }
  };

  const connectOpenRouter = async () => {
    const r = await confirm({
      title: "Connect OpenRouter",
      body: "Your browser opens openrouter.ai, where you sign in and allow Borealis Party a key of its own. What you ask the assistant, and what its tools read for it (your parties, encounters, bestiary and homebrew), goes to OpenRouter, under its own terms.",
      confirmLabel: "Continue",
    });
    if (r !== "confirm") return;
    setBusy(true);
    try {
      const { url } = await startOpenRouter({ consent: true, autoApply });
      setSigningIn({ url, before: saved?.consentedAt ?? null });
      await openExternal(url);
    } catch (e) { toast.push(errorMessage(e), "error"); } finally { setBusy(false); }
  };

  const forgetKey = async () => {
    const r = await confirm({ title: "Remove the key", body: "The server deletes your saved key. The assistant stops working until you enter one again.", confirmLabel: "Remove key", danger: true });
    if (r === "confirm") void act(removeAiKey, "Your key is removed.");
  };
  const forgetAll = async () => {
    const r = await confirm({ title: "Remove your AI", body: "The server deletes your provider settings and your key.", confirmLabel: "Remove", danger: true });
    if (r === "confirm") void act(removeAi, "Your AI is removed.");
  };

  const to = providerName(provider, mode, provider === "openai" ? baseUrl : null);
  return (
    <Card title="Your AI">
      <p className="muted">Bring your own AI model for the assistant: building encounters, homebrew and custom indexes.</p>
      {!canStore && (
        <Alert tone="warning">
          {status?.keys.why}
          {status?.keys.adminHelp && <span className="ai-admin-help"> For admins: {status.keys.adminHelp}</span>}
        </Alert>
      )}
      {offersOpenRouter && (
        <div className="ai-field ai-signin">
          <div className="party-inline">
            <Button disabled={busy || !canStore || signingIn !== null} onClick={() => void connectOpenRouter()}>Connect OpenRouter</Button>
          </div>
          {signingIn ? (
            <>
              <p className="muted ai-hint" role="status">Finish signing in to OpenRouter in your browser; this page updates by itself.</p>
              <div className="party-inline">
                <Button onClick={() => void openExternal(signingIn.url)}>Open OpenRouter again</Button>
                <Button onClick={() => setSigningIn(null)}>Stop waiting</Button>
              </div>
            </>
          ) : (
            <p className="muted ai-hint">Sign in at OpenRouter instead of pasting a key. The key is yours: you can change its limits or revoke it on openrouter.ai.</p>
          )}
        </div>
      )}
      <form className="party-form ai-form" onSubmit={save}>
        <label className="ai-field">
          <span>Provider</span>
          <ComboBox aria-label="Provider" options={PROVIDERS} value={provider} onChange={(v) => changeProvider(v as ProviderKind)} />
        </label>
        {/* The whole line is the link, so it is a full-size tap target on a phone. */}
        <a className="muted ai-hint ai-claude-hint" href="#/account" onClick={toAiApps} style={{ display: "flex", alignItems: "center", minHeight: 48 }}>
          <span>Using Claude? Connect claude.ai or Claude Desktop to Borealis Party instead (<u>Account → AI apps</u>).</span>
        </a>
        {provider === "openai" && (
          <>
            <label className="ai-field">
              <span>Start from</span>
              <ComboBox aria-label="Start from" options={[{ value: "", label: "Choose…" }, ...PRESETS.map((p) => ({ value: p.label, label: p.label }))]} value="" onChange={preset} />
            </label>
            <div className="ai-field" role="radiogroup" aria-label="How the model is reached">
              <span>How the model is reached</span>
              <label className="ai-check"><input type="radio" name="ai-mode" checked={mode === "server"} onChange={() => setMode("server")} /> Through the server (an address on the internet)</label>
              <label className="ai-check"><input type="radio" name="ai-mode" checked={mode === "device"} onChange={() => setMode("device")} /> On this device or home network (Ollama, LM Studio)</label>
            </div>
            <label className="ai-field">
              <span>Address</span>
              <input className="input" aria-label="Address" inputMode="url" autoCapitalize="none" spellCheck={false} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} required />
            </label>
            {mode === "device" && (
              <p className="muted ai-hint">This app talks to the model; the server never does. Ollama needs OLLAMA_ORIGINS to allow this app; in LM Studio turn on CORS. In a browser only localhost works; the apps also reach your home network.</p>
            )}
          </>
        )}
        <label className="ai-field">
          <span>Model</span>
          <input className="input" aria-label="Model" autoCapitalize="none" spellCheck={false} value={model} onChange={(e) => setModel(e.target.value)} required />
        </label>
        {mode === "server" && (
          <>
            {saved && sameProvider && saved.hasKey && !saved.keyReadable && <Alert tone="warning">Your saved key can't be read any more (the server's key changed). Enter it again.</Alert>}
            <label className="ai-field">
              <span>API key</span>
              <input className="input" type="password" aria-label="API key" autoComplete="off" spellCheck={false} disabled={!canStore}
                placeholder={sameProvider && saved?.hasKey ? `•••• ${saved.keyLast4 ?? ""} saved. Leave empty to keep it` : "Paste your key"}
                value={key} onChange={(e) => setKey(e.target.value)} />
            </label>
          </>
        )}
        <div className="party-line">
          <span className="grow">Apply drafts without asking</span>
          <ToggleSwitch label="Apply drafts without asking" checked={autoApply} onChange={setAutoApply} />
        </div>
        <p className="muted ai-hint">Drafts are encounters and homebrew only you see. The assistant never touches a live combat.</p>
        <label className="ai-field">
          <span>Model size, for simulations</span>
          <ComboBox aria-label="Model size" options={PROFILES} value={profile} onChange={(v) => setProfile(v as "" | Profile)} />
        </label>
        <label className="ai-check ai-consent">
          <CheckBox checked={consent} onChange={(e) => setConsent(e.target.checked)} aria-label="I agree my requests go to the provider" />
          <span>I agree that what I ask the assistant, and what its tools read for it (my parties, encounters, bestiary and homebrew), goes to {to}, under its own terms.</span>
        </label>
        <div className="party-inline">
          <Button type="submit" variant="primary" disabled={busy || !consent}>Save</Button>
          <Button disabled={busy || !saved} onClick={() => void test()}>Test connection</Button>
          {saved?.hasKey && <Button disabled={busy} onClick={() => void forgetKey()}>Remove key</Button>}
          {saved && <Button variant="danger" disabled={busy} onClick={() => void forgetAll()}>Remove</Button>}
        </div>
      </form>
      {status && (
        <p className="muted ai-hint">
          Today on this server: {status.limits.tokensToday.toLocaleString()} of {status.limits.tokensPerDay.toLocaleString()} tokens, at most {status.limits.requestsPerMinute} requests a minute.
        </p>
      )}
    </Card>
  );
}
