import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";
import { useToast } from "../../../src/components/Toast";
import { combatDeps } from "../../api/overrides/combat";
import { CombatClient, defaultClientDeps, type ClientDeps, type ClientState } from "./client";
import { browserSharing, mockCombatServer, mockWanted } from "./mockServer";
import type { Command } from "./types";

// Trying the tracker before the server has its routes: the mock answers them in this browser, shared by its tabs
// (./mockServer.ts).
if (typeof window !== "undefined" && mockWanted()) Object.assign(combatDeps, mockCombatServer(undefined, browserSharing()).deps);

/** Tests replace the client's clock and timers. */
export const clientOverrides: { deps?: Partial<ClientDeps> } = {};

/** One combat, live: its state, the connection, and `send` for every write. */
export function useCombat(id: string) {
  const toast = useToast();
  const client = useMemo(
    () => new CombatClient(id, { ...defaultClientDeps((m, tone) => toast.push(m, tone)), ...clientOverrides.deps }),
    // The toast API is stable for the page's life; a new id is a new client.
    [id],
  );
  useEffect(() => {
    client.open();
    return () => client.close();
  }, [client]);
  const state = useSyncExternalStore<ClientState>(
    useCallback((cb) => client.subscribe(cb), [client]),
    () => client.snapshot,
  );
  const send = useCallback((command: Command) => client.send(command), [client]);
  // The screen shows the optimistic state: the server's with the writes on their way applied (./client.ts).
  return { ...state, view: state.shown, server: state.view, client, send, accepting: client.accepting };
}
