import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createHttpApi } from "../http";
import type { ServerSession } from "../session";
import { BUSY_MESSAGE, FIRST_WAIT_MS, jobDeps, MAX_MISSES, MAX_WAIT_MS, resetJobs } from "./jobs";

const session: ServerSession = {
  baseUrl: "http://192.168.1.20:8787", token: "t",
  user: { id: "u", username: "alice", email: null, admin: false, mustChangePassword: false },
};

type Reply = unknown | (() => unknown);
interface Call { path: string; body: unknown; at: number }

/** A fake server: each call to a path takes the next reply in its list (the last one repeats). */
function server(replies: Record<string, Reply[]>): Call[] {
  const calls: Call[] = [];
  jobDeps.fetch = async (_s, _m, path, body) => {
    calls.push({ path, body, at: Date.now() });
    const list = replies[path];
    if (!list) throw new Error(`unexpected ${path}`);
    const reply = list.length > 1 ? list.shift() : list[0];
    return typeof reply === "function" ? (reply as () => unknown)() : reply;
  };
  return calls;
}

const START = "/api/rpc/job_start";
const STATUS = "/api/rpc/job_status";
const started = (jobId = "j1", extra: Record<string, unknown> = {}) => ({ jobId, command: "update_indexes", started: true, same: true, ...extra });
const running = (jobId = "j1") => ({ jobId, command: "update_indexes", state: "running" });
const fail = (code: string, message = code) => () => { throw { code, message }; };

/** Let the job run for `ms` of fake time. */
const run = (ms: number) => vi.advanceTimersByTimeAsync(ms);

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => {
  resetJobs();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("long downloads as background jobs", () => {
  it("starts a job and answers its result once it is done, asking less and less often", async () => {
    const result = { updated: ["core"], failed: [] };
    const calls = server({
      [START]: [started()],
      [STATUS]: [running(), running(), running(), running(), running(), running(), running(), { ...running(), state: "done", result }],
    });
    const answer = createHttpApi(session).updateIndexes();
    await run(60_000);
    await expect(answer).resolves.toEqual(result);
    expect(calls[0]).toMatchObject({ path: START, body: { command: "update_indexes", args: {} } });
    const waits = calls.slice(1).map((c, i, all) => c.at - (i === 0 ? calls[0]!.at : all[i - 1]!.at));
    expect(waits[0]).toBe(FIRST_WAIT_MS);
    expect(waits[1]).toBe(750);
    expect(waits.every((w, i) => i === 0 || w >= waits[i - 1]!)).toBe(true);
    expect(Math.max(...waits)).toBe(MAX_WAIT_MS);
    expect(calls.slice(1).every((c) => c.path === STATUS && (c.body as { jobId: string }).jobId === "j1")).toBe(true);
  });

  it("does not answer before the job is done", async () => {
    server({ [START]: [started()], [STATUS]: [running()] });
    let settled = false;
    void createHttpApi(session).updateIndexes().finally(() => { settled = true; });
    await run(30 * 60_000);
    expect(settled).toBe(false);
  });

  it("passes the args, for the 5etools downloads too", async () => {
    const calls = server({
      [START]: [{ ...started(), command: "stage_5etools_url" }],
      [STATUS]: [{ jobId: "j1", command: "stage_5etools_url", state: "done", result: { token: "x" } }],
    });
    const api = createHttpApi(session);
    const answer = api.stage5etoolsUrl({ url: "https://github.com/example/srd-data" });
    await run(1000);
    await expect(answer).resolves.toEqual({ token: "x" });
    expect(calls[0]!.body).toEqual({ command: "stage_5etools_url", args: { url: "https://github.com/example/srd-data" } });

    const update = server({
      [START]: [{ ...started(), command: "update_5etools_url" }],
      [STATUS]: [{ jobId: "j1", command: "update_5etools_url", state: "done", result: { changed: true } }],
    });
    const updated = api.update5etoolsUrl({ url: "https://example.com/5e" });
    await run(1000);
    await expect(updated).resolves.toEqual({ changed: true });
    expect(update[0]!.body).toEqual({ command: "update_5etools_url", args: { url: "https://example.com/5e" } });
  });

  it("a failed job rejects with the command's own error", async () => {
    const error = { code: "invalid_argument", message: "That address is on the server's own network." };
    server({ [START]: [started()], [STATUS]: [running(), { ...running(), state: "failed", error }] });
    const answer = createHttpApi(session).updateIndexes();
    const seen = expect(answer).rejects.toEqual(error);
    await run(5000);
    await seen;
  });

  it("a job the server restarted during rejects with that message", async () => {
    const error = { code: "interrupted", message: "The server restarted during this import; run it again." };
    server({ [START]: [started()], [STATUS]: [fail("unavailable"), { ...running(), state: "failed", error }] });
    const answer = createHttpApi(session).updateIndexes();
    const seen = expect(answer).rejects.toEqual(error);
    await run(10_000);
    await seen;
  });

  it("keeps asking through dropped connections, proxy timeouts and a restarting server", async () => {
    server({
      [START]: [started()],
      [STATUS]: [fail("network"), fail("timeout"), fail("unavailable"), running(), { ...running(), state: "done", result: 7 }],
    });
    const answer = createHttpApi(session).updateIndexes();
    await run(60_000);
    await expect(answer).resolves.toBe(7);
  });

  it("gives up after many unanswered questions in a row", async () => {
    const calls = server({ [START]: [started()], [STATUS]: [fail("network", "The server can't be reached.")] });
    const answer = createHttpApi(session).updateIndexes();
    const seen = expect(answer).rejects.toMatchObject({ code: "network" });
    await run(MAX_MISSES * MAX_WAIT_MS + 60_000);
    await seen;
    expect(calls.filter((c) => c.path === STATUS)).toHaveLength(MAX_MISSES);
  });

  it("any other failure of a question ends the wait", async () => {
    server({ [START]: [started()], [STATUS]: [fail("not_found", "That job is not on the server any more. Run it again.")] });
    const answer = createHttpApi(session).updateIndexes();
    const seen = expect(answer).rejects.toMatchObject({ code: "not_found", message: "That job is not on the server any more. Run it again." });
    await run(1000);
    await seen;
  });

  it("tries a start again when a proxy gave up on it, and never shows that 524", async () => {
    const calls = server({
      [START]: [fail("timeout", "The server took too long to answer"), fail("unavailable"), started()],
      [STATUS]: [{ ...running(), state: "done", result: "ok" }],
    });
    const answer = createHttpApi(session).updateIndexes();
    await run(10_000);
    await expect(answer).resolves.toBe("ok");
    expect(calls.filter((c) => c.path === START)).toHaveLength(3);
  });

  it("a start the server refuses is not tried again", async () => {
    const calls = server({ [START]: [fail("invalid_argument", "That command does not run as a job.")] });
    const answer = createHttpApi(session).updateIndexes();
    const seen = expect(answer).rejects.toMatchObject({ code: "invalid_argument" });
    await run(10_000);
    await seen;
    expect(calls).toHaveLength(1);
  });

  it("waits for another job of the account to end, then runs its own", async () => {
    const calls = server({
      [START]: [{ jobId: "other", command: "update_5etools_url", started: false, same: false }, started("mine")],
      [STATUS]: [
        { jobId: "other", command: "update_5etools_url", state: "running" },
        { jobId: "other", command: "update_5etools_url", state: "done", result: "not mine" },
        { ...running("mine"), state: "done", result: "mine" },
      ],
    });
    const answer = createHttpApi(session).updateIndexes();
    await run(20_000);
    await expect(answer).resolves.toBe("mine");
    expect(calls.map((c) => c.path)).toEqual([START, STATUS, STATUS, START, STATUS]);
  });

  it("joins a job that runs this very call", async () => {
    const calls = server({
      [START]: [started("j1", { started: false, same: true })],
      [STATUS]: [{ ...running(), state: "done", result: "joined" }],
    });
    const answer = createHttpApi(session).updateIndexes();
    await run(2000);
    await expect(answer).resolves.toBe("joined");
    expect(calls.filter((c) => c.path === START)).toHaveLength(1);
  });

  it("stops waiting for other jobs after a few", async () => {
    server({
      [START]: [{ jobId: "other", command: "update_5etools_url", started: false, same: false }],
      [STATUS]: [{ jobId: "other", command: "update_5etools_url", state: "done", result: null }],
    });
    const answer = createHttpApi(session).updateIndexes();
    const seen = expect(answer).rejects.toEqual({ code: "conflict", message: BUSY_MESSAGE });
    await run(60_000);
    await seen;
  });

  it("a server without jobs gets the plain call", async () => {
    server({ [START]: [fail("not_found", "The server does not offer that.")] });
    const fetch = vi.fn(async (url: string) => new Response(JSON.stringify({ plain: url }), { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    const answer = createHttpApi(session).updateIndexes();
    await run(0);
    await expect(answer).resolves.toEqual({ plain: `${session.baseUrl}/api/rpc/update_indexes` });
  });

  it("leaves every other command a plain request", async () => {
    const calls = server({});
    const fetch = vi.fn(async () => new Response("[]", { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    await createHttpApi(session).listIndexes();
    expect(calls).toEqual([]);
    expect(fetch).toHaveBeenCalledWith(`${session.baseUrl}/api/rpc/list_indexes`, expect.anything());
  });
});
