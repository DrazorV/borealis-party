import { type ReactNode, useEffect, useMemo, useSyncExternalStore } from "react";
import type { Api } from "../../src/api/contract";
import { ApiProvider, useApi } from "../../src/state/ApiContext";
import { contentSync } from "./contentSync";

/**
 * Upstream's Additional Content sections, made to fetch their lists again when the content changed (./contentSync.ts).
 * Upstream loads each list with `useAsync(..., [api])`, so the sections get an API of their own (the same commands)
 * that is new each time the lists should be fetched: their state stays (nothing is mounted again), their lists reload.
 * `hold`: follow only changes that did not come from an index update (upstream's Index files reloads after its own).
 * While mounted, the account's event stream is followed for the server's content changes.
 */
export function ContentScope({ children, hold = false }: { children: ReactNode; hold?: boolean }) {
  const api = useApi();
  const snapshot = useSyncExternalStore(contentSync.subscribe, contentSync.get);
  const version = hold ? snapshot.loud : snapshot.version;
  useEffect(() => contentSync.watch(), []);
  // A new object each version, with the page's API as its prototype: every command and `subscribe` are the same.
  const scoped = useMemo(() => Object.create(api) as Api, [api, version]);
  return <ApiProvider api={scoped}>{children}</ApiProvider>;
}

/** The content version, for party sections that load their own lists. */
export function useContentVersion(): number {
  return useSyncExternalStore(contentSync.subscribe, contentSync.get).version;
}
