import { type ReactNode, useState } from "react";
import { Button } from "../../src/components/Button";
import { TabPanel, Tabs } from "../../src/components/Tabs";

/**
 * Account → AI apps: how to connect ChatGPT, Claude and the developer tools (0.3.2), and the instructions connected
 * apps get. Menu names follow each vendor's own docs as read on 2026-11-14 (party/docs/ai-auth.md lists the pages);
 * vendors rename menus, so the guides say what to search for.
 */

export type GuideId = "chatgpt" | "claude" | "dev";

const TABS: { id: GuideId; label: string }[] = [
  { id: "chatgpt", label: "ChatGPT" },
  { id: "claude", label: "Claude" },
  { id: "dev", label: "Developer tools" },
];

export const EXAMPLE_PROMPTS = [
  "Build a forest ambush for my level 5 party from my bestiary.",
  "Simulate my encounter \"Bridge ambush\" 10 times and tell me what happened.",
  "Draft a homebrew magic item, a cloak that turns aside arrows, and check it.",
];

const Code = ({ children }: { children: ReactNode }) => <code className="ai-guide-code">{children}</code>;

function Prompts() {
  return (
    <>
      <p className="ai-guide-sub">Then try:</p>
      <ul className="ai-guide-prompts">
        {EXAMPLE_PROMPTS.map((p) => <li key={p}>“{p}”</li>)}
      </ul>
    </>
  );
}

function Guide({ id, url, oauth }: { id: GuideId; url: string; oauth: boolean }) {
  if (id === "chatgpt") {
    return (
      <>
        <ol className="ai-guide-steps">
          <li>In ChatGPT on the web, open <strong>Plugins</strong> and choose <strong>+</strong>, then <strong>Add custom MCP server</strong>.</li>
          <li>Name: <Code>Borealis Party</Code>. Server URL: the address above.</li>
          <li>Authentication: <strong>OAuth</strong>. Choose <strong>I understand and want to continue</strong>, then <strong>Create as a plugin</strong> and install it.</li>
          <li>Borealis Party opens: sign in if asked, and approve what the app may do.</li>
          <li>In a chat, type <Code>@</Code> and pick <strong>Borealis Party</strong>.</li>
        </ol>
        <p className="muted ai-guide-note">Custom MCP servers need a ChatGPT plan that allows them; on a workspace plan, an admin may have to add it. Paste the instructions below into a Project's instructions too, for a project that always uses it.</p>
        {!oauth && <p className="muted ai-guide-note">This server has no public address, so ChatGPT can't sign in to it.</p>}
        <Prompts />
      </>
    );
  }
  if (id === "claude") {
    return (
      <>
        <ol className="ai-guide-steps">
          <li>In claude.ai or Claude Desktop, open <strong>Customize</strong>, then <strong>Connectors</strong>.</li>
          <li>Choose <strong>+ Add</strong>, then <strong>Add custom connector</strong>.</li>
          <li>Name: <Code>Borealis Party</Code>. Remote MCP server URL: the address above. Choose <strong>Continue</strong>, sign in now, and <strong>Add</strong>.</li>
          <li>Borealis Party opens: sign in if asked, and approve what the app may do.</li>
          <li>In a chat, choose <strong>+</strong>, then <strong>Connectors</strong>, and turn on <strong>Borealis Party</strong>.</li>
        </ol>
        <p className="muted ai-guide-note">On a Team or Enterprise plan, an owner adds it under Organization settings, Connectors, and members choose Connect. The free plan allows one custom connector.</p>
        {!oauth && <p className="muted ai-guide-note">This server has no public address, so Claude can't sign in to it.</p>}
        <Prompts />
      </>
    );
  }
  return (
    <>
      <ol className="ai-guide-steps">
        <li>Claude Code: <Code>claude mcp add --transport http borealis-party {url}</Code>, then <Code>/mcp</Code> in Claude Code to sign in.</li>
        <li>Codex: <Code>codex mcp add borealis-party --url {url}</Code>, then <Code>codex mcp login borealis-party</Code>.</li>
        <li>Cursor, Gemini CLI and other MCP clients: make a key under <strong>Keys for developer tools</strong> below; it shows the setup to paste.</li>
      </ol>
      {!oauth && <p className="muted ai-guide-note">Without a public address, signing in doesn't work here: use a key.</p>}
      <Prompts />
    </>
  );
}

/** The guides, in tabs. */
export function ConnectGuides({ url, oauth }: { url: string; oauth: boolean }) {
  const [tab, setTab] = useState<GuideId>("chatgpt");
  return (
    <div className="ai-guides">
      <h3 className="ai-head">Connect an app</h3>
      <Tabs tabs={TABS} active={tab} onChange={(id) => setTab(id as GuideId)} label="Connection guides" idPrefix="ai-guide" />
      <TabPanel active={tab} idPrefix="ai-guide">
        <Guide id={tab} url={url} oauth={oauth} />
      </TabPanel>
      <p className="muted ai-guide-note">Menu names change; if one isn't there, search the app's settings for “custom MCP server” or “custom connector”.</p>
    </div>
  );
}

/** The instructions connected apps get, read-only, with Copy. */
export function AssistantInstructions({ text, onCopy }: { text: string; onCopy: (text: string) => void }) {
  return (
    <div className="party-field ai-instructions">
      <h3 className="ai-head">Instructions for the assistant</h3>
      <span className="party-note muted">Connected apps get these automatically; for ChatGPT, also paste them into a Project's instructions.</span>
      <textarea className="input textarea ai-snippet ai-instructions-text" readOnly rows={12} aria-label="Instructions for the assistant" value={text} />
      <div className="party-inline"><Button onClick={() => onCopy(text)}>Copy instructions</Button></div>
    </div>
  );
}
