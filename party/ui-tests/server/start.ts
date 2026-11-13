/**
 * Global setup: a throwaway party server for the phone UI tests. It gets a new temporary config folder (SQLite
 * database, data folder), a free port on 127.0.0.1, the built web app (`ui/party/dist`) and the repository's content,
 * and two seeded accounts. It never reads or writes any other server's data. Teardown stops it and removes the folder.
 *
 * Which server binary runs:
 *  - `BOREALIS_SERVER_BIN`: a native binary (Linux in CI, or a Windows build).
 *  - `BOREALIS_SERVER_WSL_BIN`: a Linux binary inside WSL, started through `wsl.exe` (Windows only).
 *  - otherwise `party/target/{debug,release}/borealis-server[.exe]`, or `$CARGO_TARGET_DIR/...`, whichever exists.
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { ACCOUNTS, SERVER_FILE, type ServerFile } from "../tests/accounts";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const web = process.env.BOREALIS_WEB_DIR ?? join(repo, "ui/party/dist");
const content = join(repo, "content");

function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once("error", fail);
    s.listen(0, "127.0.0.1", () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => ok(port));
    });
  });
}

/** `C:\x\y` as WSL sees it: `/mnt/c/x/y`. */
function wslPath(p: string): string {
  const m = /^([A-Za-z]):[\\/](.*)$/.exec(p);
  return m ? `/mnt/${m[1].toLowerCase()}/${m[2].replace(/\\/g, "/")}` : p.replace(/\\/g, "/");
}

function nativeBinary(): string | null {
  if (process.env.BOREALIS_SERVER_BIN) return process.env.BOREALIS_SERVER_BIN;
  const exe = process.platform === "win32" ? "borealis-server.exe" : "borealis-server";
  const targets = [process.env.CARGO_TARGET_DIR, join(repo, "party/target")].filter(Boolean) as string[];
  for (const t of targets) for (const profile of ["debug", "release"]) {
    const p = join(t, profile, exe);
    if (existsSync(p)) return p;
  }
  return null;
}

async function waitUp(url: string, child: ChildProcess, log: () => string): Promise<void> {
  const until = Date.now() + 90_000;
  while (Date.now() < until) {
    if (child.exitCode !== null) throw new Error(`The test server stopped (exit ${child.exitCode}).\n${log()}`);
    try {
      const r = await fetch(`${url}/api/server`);
      if (r.ok) return;
    } catch { /* not listening yet */ }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`The test server did not answer at ${url} within 90 s.\n${log()}`);
}

export default async function globalSetup(): Promise<() => Promise<void>> {
  if (!existsSync(join(web, "index.html"))) {
    throw new Error(`No web app at ${web}. Build it first: from ui/, node party/build.mjs`);
  }
  const port = await freePort();
  const url = `http://127.0.0.1:${port}`;
  let output = "";
  const log = () => output.slice(-4000);
  let child: ChildProcess;
  let cleanup = () => {};

  const wslBin = process.env.BOREALIS_SERVER_WSL_BIN;
  if (wslBin) {
    child = spawn("wsl.exe", [
      "-e", "bash", wslPath(join(here, "wsl-server.sh")), wslBin, String(port), wslPath(web), wslPath(content),
      ACCOUNTS.admin.password, ACCOUNTS.player.password,
    ], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  } else {
    const bin = nativeBinary();
    if (!bin) {
      throw new Error(
        "No party server binary. Build it (from party/: cargo build -p borealis-server) or set BOREALIS_SERVER_BIN, " +
        "or on Windows BOREALIS_SERVER_WSL_BIN to a Linux build inside WSL. See party/README.md, Phone UI tests.",
      );
    }
    const dir = mkdtempSync(join(tmpdir(), "borealis-ui-tests-"));
    cleanup = () => rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 });
    const env = {
      ...process.env,
      BOREALIS_CONFIG_DIR: dir,
      BOREALIS__SERVER__BIND: `127.0.0.1:${port}`,
      BOREALIS__PATHS__WEB: web,
      BOREALIS__PATHS__CONTENT: content,
      BOREALIS__DATABASE__TYPE: "sqlite",
      BOREALIS__LOG__LEVEL: process.env.BOREALIS__LOG__LEVEL ?? "warn",
    };
    for (const [name, a] of Object.entries(ACCOUNTS)) {
      const args = ["add-user", name, "--email", a.email, ...(a.admin ? ["--admin"] : [])];
      const r = spawnSync(bin, args, { env, input: `${a.password}\n`, encoding: "utf8" });
      if (r.status !== 0) {
        cleanup();
        throw new Error(`add-user ${name} failed: ${r.stderr || r.stdout || r.error}`);
      }
    }
    child = spawn(bin, [], { env, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  }
  child.stdout?.on("data", (d) => { output += d; });
  child.stderr?.on("data", (d) => { output += d; });

  try {
    await waitUp(url, child, log);
  } catch (e) {
    child.kill();
    cleanup();
    throw e;
  }
  const file: ServerFile = { url };
  writeFileSync(SERVER_FILE, JSON.stringify(file));

  return async () => {
    if (child.exitCode === null) {
      const exited = new Promise((ok) => child.once("exit", ok));
      if (child.stdin) child.stdin.end(); // wsl-server.sh stops its server and removes its folder
      else child.kill();
      const done = await Promise.race([exited.then(() => true), new Promise((ok) => setTimeout(() => ok(false), 10_000))]);
      if (!done) child.kill();
    }
    rmSync(SERVER_FILE, { force: true });
    cleanup();
  };
}
