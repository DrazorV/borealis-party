import type { ApiError, CommandName } from "../../../src/api/contract";
import { serverFetch, unavailable } from "../http";
import type { ServerSession } from "../session";
import { override, type Method } from "./registry";

/**
 * The long downloads as background jobs (plan I1, party/crates/borealis-server/src/jobs.rs).
 *
 * Through Cloudflare a request that takes over about 100 s ends in a 524 while the server keeps working, and a first
 * index download takes over three minutes. So these commands start a job (`job_start`, answered at once) and ask
 * about it (`job_status`) until it ends, then answer or fail exactly like the plain call. The pages (Indexes, 5etools,
 * the Import section) call them as before; progress still comes as the commands' own events.
 * - A wait between questions grows from half a second to five seconds.
 * - A question that gets no answer (the network, a proxy, a restarting server) is asked again for a few minutes: the
 *   job goes on meanwhile.
 * - The server runs one job per account. If another one is running (from another device, say), this waits for it to
 *   end and then starts its own.
 * - A server older than jobs answers "does not offer that": the plain call is made instead.
 */

/** The commands that run as jobs, and their server names. */
export const JOB_COMMANDS = {
  updateIndexes: "update_indexes",
  stage5etoolsUrl: "stage_5etools_url",
  update5etoolsUrl: "update_5etools_url",
} as const satisfies Partial<Record<CommandName, string>>;

export const FIRST_WAIT_MS = 500;
export const MAX_WAIT_MS = 5000;
/** Questions in a row that may go unanswered before the job is given up on (about four minutes at the longest wait). */
export const MAX_MISSES = 50;
/** Times a start that got no answer is tried again. */
export const START_TRIES = 3;
/** Other jobs waited for before this one gives up. */
const MAX_WAITS_FOR_OTHERS = 5;

export const BUSY_MESSAGE = "Other downloads keep running on this account. Try again when they have finished.";

/** Failures that say nothing about the job: no answer, a proxy that gave up, a server restarting. */
const TRANSIENT = new Set(["network", "timeout", "unavailable"]);

/** `same`: the job runs this very call (started by it, or by an earlier try of it that got no answer). */
interface Started { jobId: string; command: string; started: boolean; same: boolean }
interface Status { jobId: string; command: string; state: "running" | "done" | "failed"; result?: unknown; error?: ApiError; progress?: unknown }

export interface JobDeps {
  fetch: (session: ServerSession, method: string, path: string, body?: unknown) => Promise<unknown>;
  sleep: (ms: number) => Promise<void>;
}

const DEFAULT_DEPS: JobDeps = {
  // Late-bound: ../http.ts imports this module before its own exports are set.
  fetch: (session, method, path, body) => serverFetch(session, method, path, body),
  sleep: (ms) => new Promise((resolve) => { setTimeout(resolve, ms); }),
};

export const jobDeps: JobDeps = { ...DEFAULT_DEPS };

/** Tests: start over. */
export function resetJobs(): void {
  Object.assign(jobDeps, DEFAULT_DEPS);
}

const isTransient = (e: unknown) => TRANSIENT.has(String((e as ApiError | null)?.code));
const notOffered = (e: unknown) =>
  (e as ApiError | null)?.code === "not_found" && (e as ApiError).message === "The server does not offer that.";

/** Thrown inside {@link runJob} when the server has no jobs: the caller makes the plain call. */
const NO_JOBS = Symbol("no jobs");

async function start(session: ServerSession, command: string, args: Record<string, unknown>, deps: JobDeps): Promise<Started> {
  for (let attempt = 1; ; attempt++) {
    try {
      return await deps.fetch(session, "POST", "/api/rpc/job_start", { command, args }) as Started;
    } catch (e) {
      if (notOffered(e)) throw NO_JOBS;
      // A start is safe to repeat: while the first one runs, the server answers that one.
      if (!isTransient(e) || attempt >= START_TRIES) throw e;
      await deps.sleep(FIRST_WAIT_MS * 2 ** attempt);
    }
  }
}

/** Ask about the job until it ends. */
async function wait(session: ServerSession, jobId: string, deps: JobDeps): Promise<Status> {
  let delay = FIRST_WAIT_MS;
  let misses = 0;
  for (;;) {
    await deps.sleep(delay);
    delay = Math.min(MAX_WAIT_MS, Math.round(delay * 1.5));
    let status: Status;
    try {
      status = await deps.fetch(session, "POST", "/api/rpc/job_status", { jobId }) as Status;
    } catch (e) {
      if (!isTransient(e) || ++misses >= MAX_MISSES) throw e;
      continue;
    }
    misses = 0;
    if (status.state !== "running") return status;
  }
}

/** Run `command` as a job and answer what the plain call would have. */
export async function runJob(session: ServerSession, command: string, args: Record<string, unknown>, deps: JobDeps = jobDeps): Promise<unknown> {
  for (let others = 0; ; others++) {
    const job = await start(session, command, args, deps);
    const status = await wait(session, job.jobId, deps);
    if (!job.same) {
      // Another job of the account ended: now start this one.
      if (others < MAX_WAITS_FOR_OTHERS) continue;
      throw { code: "conflict", message: BUSY_MESSAGE } as unknown as ApiError;
    }
    if (status.state === "done") return status.result;
    const error = status.error ?? ({ code: "internal", message: "The job failed." } as ApiError);
    throw notOffered(error) ? unavailable() : error;
  }
}

override("jobs", (methods, session) => {
  for (const [name, command] of Object.entries(JOB_COMMANDS) as [CommandName, string][]) {
    const plain: Method = methods[name];
    methods[name] = async (args?: Record<string, unknown>) => {
      try {
        return await runJob(session, command, args ?? {});
      } catch (e) {
        if (e === NO_JOBS) return plain(args);
        throw e;
      }
    };
  }
});
