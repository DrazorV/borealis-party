import { serverFetch } from "../http";
import { loadSession, type ServerSession } from "../session";
import { override } from "./registry";

/**
 * Share codes (plan E4, E6, D37; server: party/crates/borealis-server/src/shares.rs). The server builds the bundle from
 * this account's folder and keeps it; another player redeems the code, which stages the file in their own folder, and
 * upstream's Open bundle window previews it from the token as for any bundle (with the 0.1.4 safe receive). This
 * module holds the calls, and tells the server once that window accepted a share's bundle, so its file can go.
 */

type Session = Pick<ServerSession, "baseUrl" | "token">;

/** A source address that stands in for packs: the recipient adds it again through the server. */
export interface ShareLink {
  kind: "index" | "5etools";
  url: string;
  packs: string[];
}

export type ShareStatus = "pending" | "claimed" | "done" | "declined" | "cancelled" | "expired";

/** A share as its sender sees it. */
export interface SentShare {
  id: string;
  fileName: string;
  characters: number;
  packs: number;
  links: ShareLink[];
  bytes: number;
  createdAt: number;
  expiresAt: number;
  status: ShareStatus;
  forUsername: string | null;
  claimedBy: string | null;
}

/** A share as its recipient sees it. */
export interface ReceivedShare {
  id: string;
  from: string;
  fromId: string;
  fileName: string;
  characters: number;
  packs: number;
  links: ShareLink[];
  bytes: number;
  expiresAt: number;
  status: ShareStatus;
}

export interface ShareLimits {
  openPerSender: number;
  waitingPerSender: number;
  days: number;
  maxBytes: number;
  diskBytes: number;
  heldBytes: number;
}

export interface ShareList {
  sent: SentShare[];
  received: ReceivedShare[];
  blocked: { id: string; username: string }[];
  limits: ShareLimits;
}

export interface NewShare {
  characterIds: string[];
  packs: "required" | "all" | "none";
  includeFiles?: boolean;
  forUsername?: string;
}

export interface MadeShare {
  share: SentShare;
  /** `ABCD-EFGH-JKMN-PQRS`. */
  code: string;
  /** The page that redeems it, when the server knows its public address. */
  link: string | null;
  /** Packs left out: licensed, with no address that can travel. */
  leftOut: string[];
}

/** A redeemed share, and its staged bundle (none when every item travels as a link). */
export interface Redeemed {
  share: ReceivedShare;
  bundle: { token: string; name: string } | null;
}

export interface AddedLinks {
  indexes: { url: string; packs: string[]; added: boolean; already?: boolean; message?: string }[];
  fivetools: { url: string; packs: string[]; allowed: boolean; message?: string }[];
}

function session(): Session {
  const s = loadSession();
  if (!s) throw { code: "unauthorized", message: "Please sign in." };
  return s;
}

const call = <T>(method: string, path: string, body?: unknown) => serverFetch(session(), method, path, body) as Promise<T>;

export const createShare = (share: NewShare) => call<MadeShare>("POST", "/api/shares", share);
export const listShares = () => call<ShareList>("GET", "/api/shares");
export const cancelShare = (id: string) => call<unknown>("DELETE", `/api/shares/${encodeURIComponent(id)}`);
export const redeemShare = (code: string) => watchStaged(call<Redeemed>("POST", "/api/shares/redeem", { code }));
export const reopenShare = (id: string) => watchStaged(call<Redeemed>("POST", `/api/shares/${encodeURIComponent(id)}/open`, {}));
export const declineShare = (id: string, block: boolean) => call<unknown>("POST", `/api/shares/${encodeURIComponent(id)}/decline`, { block });
export const addShareLinks = (id: string) => call<AddedLinks>("POST", `/api/shares/${encodeURIComponent(id)}/links`, {});
export const unblockSender = (id: string) => call<unknown>("DELETE", `/api/shares/blocks/${encodeURIComponent(id)}`);
export const acceptShare = (id: string) => call<unknown>("POST", `/api/shares/${encodeURIComponent(id)}/accepted`, {});

/** What the Open bundle window did with a share's bundle. */
export type ShareOutcome = "accepted" | "cancelled";

const byToken = new Map<string, string>();
const byBundle = new Map<string, string>();
const listeners = new Set<(shareId: string, outcome: ShareOutcome) => void>();

/** Hear how the Open bundle window closed for a share (the receive page). */
export function onShareOutcome(listener: (shareId: string, outcome: ShareOutcome) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

async function watchStaged(redeemed: Promise<Redeemed>): Promise<Redeemed> {
  const out = await redeemed;
  if (out.bundle) byToken.set(out.bundle.token, out.share.id);
  return out;
}

function settle(bundleId: string, outcome: ShareOutcome): string | undefined {
  const id = byBundle.get(bundleId);
  byBundle.delete(bundleId);
  if (id) listeners.forEach((l) => l(id, outcome));
  return id;
}

/** For tests: forget every staged share. */
export function resetShareWatchForTests(): void {
  byToken.clear();
  byBundle.clear();
  listeners.clear();
}

override("shares", (methods) => {
  const open = methods.bundleOpen;
  const accept = methods.bundleAccept;
  const discard = methods.bundleDiscard;
  methods.bundleOpen = async (args?: Record<string, unknown>) => {
    const token = String(args?.token ?? "");
    const preview = await open(args) as { bundleId: string };
    const id = byToken.get(token);
    if (id) {
      byToken.delete(token);
      byBundle.set(preview.bundleId, id);
    }
    return preview;
  };
  methods.bundleAccept = async (args?: Record<string, unknown>) => {
    const bundleId = String(args?.bundleId ?? "");
    // A share's bundle is always another player's: the server's foreign check applies whatever the window sent.
    const out = await accept(byBundle.has(bundleId) ? { ...args, fromAnotherPlayer: true } : args);
    const id = byBundle.get(bundleId);
    // The server forgets the share's file; if that call fails, the share just expires as usual.
    if (id) await acceptShare(id).catch(() => undefined);
    settle(bundleId, "accepted");
    return out;
  };
  methods.bundleDiscard = async (args?: Record<string, unknown>) => {
    // Closed without a choice: the share waits in the list, to open again or decline.
    settle(String(args?.bundleId ?? ""), "cancelled");
    return await discard(args);
  };
});
