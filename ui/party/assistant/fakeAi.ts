import type { AiStatus, Reply, SavedProvider, ToolInfo, ToolOutcome } from "../api/overrides/assistant";

/** Tests: the server's `/api/ai/*` routes, from canned answers. No model or provider is ever called. */

export interface Call { method: string; path: string; body: unknown }

export const TOOLS: ToolInfo[] = [
  { name: "character_list", description: "List.", inputSchema: { type: "object" }, scope: "characters:read", effect: "read" },
  { name: "bestiary_search", description: "Search.", inputSchema: { type: "object" }, scope: "bestiary:read", effect: "read" },
  { name: "encounter_create", description: "Create.", inputSchema: { type: "object" }, scope: "encounters:write", effect: "draft" },
  { name: "encounter_add_creature", description: "Add.", inputSchema: { type: "object" }, scope: "encounters:write", effect: "draft" },
  { name: "sim_start", description: "Start.", inputSchema: { type: "object" }, scope: "encounters:simulate", effect: "read" },
  { name: "sim_auto", description: "Run.", inputSchema: { type: "object" }, scope: "encounters:simulate", effect: "read" },
  { name: "sim_report", description: "Report.", inputSchema: { type: "object" }, scope: "encounters:simulate", effect: "read" },
  { name: "index_build", description: "Build.", inputSchema: { type: "object" }, scope: "homebrew:read", effect: "read" },
];

export const SAVED: SavedProvider = {
  provider: "openai", mode: "server", baseUrl: "https://api.example.com/v1", model: "gpt-test",
  hasKey: true, keyLast4: "wxyz", keyReadable: true, autoApply: false, profile: "small", consentedAt: 1,
};

export const KEYS_OFF = "Saving cloud AI keys isn't switched on for this server yet; ask its admin. A model on your own device works without it.";
export const ADMIN_HELP = "This server has no key to encrypt AI keys with, so it doesn't save them. Set BOREALIS__AI__SECRET_KEY and restart it.";

/** As `GET /api/ai` answers; `admin` adds the admin's instructions when keys can't be saved, as the server does. */
export function status(provider: SavedProvider | null, canStore = true, admin = false): AiStatus {
  return {
    keys: canStore ? { canStore } : { canStore, why: KEYS_OFF, ...(admin ? { adminHelp: ADMIN_HELP } : {}) },
    signIn: { openrouter: true },
    provider,
    scopes: ["bestiary:read", "characters:read", "encounters:read", "encounters:write", "encounters:simulate", "homebrew:read", "homebrew:write"],
    limits: { requestsPerMinute: 20, tokensPerDay: 200000, tokensToday: 1234 },
    defaults: {
      openai: { baseUrl: "https://api.openai.com/v1", model: "gpt-4.1-mini" },
      anthropic: { baseUrl: "https://api.anthropic.com", model: "claude-sonnet-5-5" },
      gemini: { baseUrl: "https://generativelanguage.googleapis.com", model: "gemini-2.5-flash" },
    },
  };
}

export interface FakeAi {
  calls: Call[];
  status: AiStatus | null;
  /** The model's answers to `/api/ai/chat`, in turn. */
  replies: Reply[];
  /** What each tool answers. */
  results: Record<string, ToolOutcome>;
  autoApply: boolean;
  fetch: (s: unknown, method: string, path: string, body?: unknown) => Promise<unknown>;
}

export const reply = (text: string, calls: [string, Record<string, unknown>][] = []): Reply => ({
  text, toolCalls: calls.map(([name, args], i) => ({ id: `c${Math.random().toString(36).slice(2)}${i}`, name, args })), inputTokens: 1, outputTokens: 1,
});

export function fakeAi(saved: SavedProvider | null = SAVED): FakeAi {
  const ai: FakeAi = {
    calls: [], status: status(saved), replies: [], results: {}, autoApply: false,
    fetch: async (_s, method, path, body) => {
      ai.calls.push({ method, path, body });
      if (!ai.status) throw { code: "not_found", message: "The server does not offer that." };
      if (path === "/api/ai" && method === "GET") return ai.status;
      if (path === "/api/ai" && method === "PUT") {
        const f = body as { provider: SavedProvider["provider"]; mode: SavedProvider["mode"]; baseUrl?: string; model: string; key?: string; autoApply: boolean; profile?: SavedProvider["profile"] };
        const before = ai.status.provider;
        ai.status = status({
          provider: f.provider, mode: f.mode, baseUrl: f.baseUrl ?? null, model: f.model,
          hasKey: f.mode === "server" && Boolean(f.key ?? before?.hasKey), keyLast4: f.key ? f.key.slice(-4) : before?.keyLast4 ?? null,
          keyReadable: true, autoApply: f.autoApply, profile: f.profile ?? "small", consentedAt: 2,
        }, ai.status.keys.canStore);
        return ai.status;
      }
      if (path === "/api/ai/key") { ai.status = status(ai.status.provider && { ...ai.status.provider, hasKey: false, keyLast4: null }); return ai.status; }
      if (path === "/api/ai" && method === "DELETE") { ai.status = status(null); return ai.status; }
      if (path === "/api/ai/oauth/openrouter/start") {
        if (!ai.status.keys.canStore) throw { code: "ai_keys_unavailable", message: KEYS_OFF };
        return { url: "https://openrouter.ai/auth?callback_url=x&code_challenge=y&code_challenge_method=S256&state=z" };
      }
      if (path === "/api/ai/test") return { ok: true, reply: "OK" };
      if (path.startsWith("/api/ai/tools?")) return { system: "SYSTEM", tools: TOOLS, autoApply: ai.autoApply };
      if (path === "/api/ai/chat") {
        const next = ai.replies.shift();
        if (!next) throw { code: "ai_provider_error", message: "No more canned replies." };
        return { reply: next, budget: { tokensToday: 2, tokensPerDay: 100 } };
      }
      const tool = /^\/api\/ai\/tools\/(.+)$/.exec(path);
      if (tool) return ai.results[decodeURIComponent(tool[1]!)] ?? { ok: true, result: { done: true } };
      throw { code: "not_found", message: `no fake for ${method} ${path}` };
    },
  };
  return ai;
}
