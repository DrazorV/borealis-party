#!/usr/bin/env node
// Times the long Borealis Party imports (plan item I0) against a server, signed in as a THROWAWAY account.
//
//   node party/tools/measure-imports.mjs --server https://party.example.com --via-cloudflare \
//     --index https://raw.githubusercontent.com/AuroraLegacy/elements/master/core.index \
//     --5etools-url https://github.com/<owner>/<repo> \
//     --aurora-zip ./aurora-content.zip
//
// It asks for the username and password itself (the password is not echoed) and never prints or saves them. Use an
// account made for this and nothing else: every step writes into that account's library, and the script removes what
// it added at the end (packs, indexes, 5etools addresses, characters a bundle brought), unless you pass --keep.
//
// Steps, each only when its option is given (the export and the bundle always run unless turned off):
//   --index URL          add_index (repeatable), then update_indexes for all of them
//                        (--repeat-update runs update_indexes a second time, when nothing has changed)
//   --5etools-url URL    stage_5etools_url, scan_5etools_folder, import_5etools on every source found
//                        (--5etools-sources A,B,... picks some; --fluff includes fluff)
//   --aurora-zip FILE    preview_import_pack + install_import_preview on a zip of Aurora content
//   (export)             bundle_export of the whole library, with what the steps above installed; its size is
//                        recorded (--save-export FILE keeps it; --no-export skips it)
//   --bundle FILE        stage_bundle + bundle_open + bundle_accept on FILE, after the cleanup of the steps above, so
//                        it lands in an empty library. Without --bundle the export just made is used
//                        (--no-bundle skips it).
//
// Other options:
//   --via-cloudflare     resolve the host over DNS-over-HTTPS (cloudflare-dns.com), so a LAN DNS record that points
//                        at the proxy is bypassed and the run goes through Cloudflare like a phone outside the home
//   --resolve IP         connect to IP instead (SNI and Host stay the server's name)
//   --user NAME          username (else asked); the password is always asked
//   --json FILE          also write the results as JSON (no credentials in it)
//   --keep               leave everything installed
//
// Non-interactive use (stdin not a terminal): the first two lines of stdin are the username and the password.
//
// Needs Node 18 or later and nothing else. A step that takes over about 80 s, or answers 524, is the trigger for
// background jobs (I1); a file over about 70 MB, or a 413, is the trigger for chunked upload (I2).

import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import http from "node:http";
import https from "node:https";

const MiB = 1024 * 1024;
const SLOW_S = 80;
const BIG_MB = 70;

// ---------- arguments ----------

function parseArgs(argv) {
  const opts = { index: [], export: true, bundle: null, useBundle: true };
  const need = (i) => {
    if (i + 1 >= argv.length) fail(`${argv[i]} needs a value`);
    return argv[i + 1];
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    switch (a) {
      case "--server": opts.server = need(i); i++; break;
      case "--index": opts.index.push(need(i)); i++; break;
      case "--repeat-update": opts.repeatUpdate = true; break;
      case "--5etools-url": opts.fiveUrl = need(i); i++; break;
      case "--5etools-sources": opts.fiveSources = need(i).split(",").map((s) => s.trim()).filter(Boolean); i++; break;
      case "--fluff": opts.fluff = true; break;
      case "--aurora-zip": opts.auroraZip = need(i); i++; break;
      case "--bundle": opts.bundle = need(i); i++; break;
      case "--no-bundle": opts.useBundle = false; break;
      case "--no-export": opts.export = false; break;
      case "--save-export": opts.saveExport = need(i); i++; break;
      case "--via-cloudflare": opts.viaCloudflare = true; break;
      case "--resolve": opts.resolve = need(i); i++; break;
      case "--user": opts.user = need(i); i++; break;
      case "--json": opts.json = need(i); i++; break;
      case "--keep": opts.keep = true; break;
      case "-h": case "--help": usage(0); break;
      default: fail(`unknown option ${a} (see --help)`);
    }
  }
  if (!opts.server) usage(1);
  opts.server = opts.server.replace(/\/+$/, "");
  return opts;
}

function usage(code) {
  const text = readFileSync(new URL(import.meta.url), "utf8").split("\n").slice(1).filter((l) => l.startsWith("//"));
  console.log(text.map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(code);
}

function fail(message) {
  console.error(`measure-imports: ${message}`);
  process.exit(2);
}

// ---------- credentials (never printed, never stored) ----------

function readStdinLines() {
  return new Promise((resolve) => {
    let data = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (c) => { data += c; });
    process.stdin.on("end", () => resolve(data.split(/\r?\n/)));
  });
}

function askLine(question) {
  return new Promise((resolve) => {
    process.stdout.write(question);
    let line = "";
    const onData = (chunk) => {
      for (const ch of chunk.toString("utf8")) {
        if (ch === "\n" || ch === "\r") { process.stdin.off("data", onData); process.stdin.pause(); process.stdout.write("\n"); resolve(line); return; }
        line += ch;
      }
    };
    process.stdin.resume();
    process.stdin.on("data", onData);
  });
}

function askHidden(question) {
  return new Promise((resolve) => {
    process.stdout.write(question);
    const stdin = process.stdin;
    stdin.setRawMode(true);
    stdin.resume();
    let secret = "";
    const done = (value) => {
      stdin.setRawMode(false);
      stdin.pause();
      stdin.off("data", onData);
      process.stdout.write("\n");
      resolve(value);
    };
    const onData = (chunk) => {
      for (const ch of chunk.toString("utf8")) {
        if (ch === "\u0003") { stdin.setRawMode(false); process.stdout.write("\n"); process.exit(130); }
        if (ch === "\r" || ch === "\n" || ch === "\u0004") { done(secret); return; }
        if (ch === "\u0008" || ch === "\u007f") { secret = secret.slice(0, -1); continue; }
        secret += ch;
      }
    };
    stdin.on("data", onData);
  });
}

async function credentials(opts) {
  if (!process.stdin.isTTY) {
    const lines = await readStdinLines();
    const user = opts.user ?? lines.shift();
    const password = lines.shift();
    if (!user || !password) fail("stdin is not a terminal: give the username and password as its first two lines");
    return { user, password };
  }
  const user = opts.user ?? (await askLine("Throwaway account username: "));
  const password = await askHidden("Password (not shown): ");
  return { user: user.trim(), password };
}

// ---------- HTTP with timings ----------

let target; // { url: URL, ip?: string }

async function dohLookup(host) {
  const res = await fetch(`https://cloudflare-dns.com/dns-query?name=${encodeURIComponent(host)}&type=A`, {
    headers: { accept: "application/dns-json" },
  });
  const body = await res.json();
  const a = (body.Answer ?? []).find((r) => r.type === 1);
  if (!a) fail(`no public A record for ${host}`);
  return a.data;
}

/** One request; resolves with status, body and timings in seconds. Never times out on its own. */
function send(method, path, { token, json, raw } = {}) {
  const body = raw ?? (json === undefined ? null : Buffer.from(JSON.stringify(json)));
  const url = new URL(path, target.url);
  const lib = url.protocol === "https:" ? https : http;
  const headers = { accept: "application/json" };
  if (body) {
    headers["content-type"] = "application/json";
    headers["content-length"] = String(body.length);
  }
  if (token) headers.authorization = `Bearer ${token}`;
  const options = { method, headers, agent: false, servername: url.hostname };
  if (target.ip) {
    const ip = target.ip;
    const family = ip.includes(":") ? 6 : 4;
    options.lookup = (_host, o, cb) => (o && o.all ? cb(null, [{ address: ip, family }]) : cb(null, ip, family));
  }
  return new Promise((resolve, reject) => {
    const t0 = performance.now();
    let tSent = null;
    const req = lib.request(url, options, (res) => {
      const tFirst = performance.now();
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => {
        const tDone = performance.now();
        const text = Buffer.concat(chunks).toString("utf8");
        let data = null;
        try { data = JSON.parse(text); } catch { /* HTML or empty */ }
        const sent = tSent ?? tFirst;
        resolve({
          status: res.statusCode,
          server: res.headers.server ?? "",
          remote: req.socket?.remoteAddress ?? "",
          text,
          data,
          upBytes: body ? body.length : 0,
          downBytes: Buffer.byteLength(text),
          uploadS: (sent - t0) / 1000,
          waitS: Math.max(0, tFirst - sent) / 1000,
          totalS: (tDone - t0) / 1000,
        });
      });
      res.on("error", reject);
    });
    req.setTimeout(0);
    req.on("error", reject);
    req.on("finish", () => { tSent = performance.now(); });
    if (body) req.end(body); else req.end();
  });
}

/** A short reason for a failed answer, never the raw HTML page. */
function reason(r) {
  if (r.data && typeof r.data.message === "string") return r.data.message.slice(0, 160);
  if (/<html/i.test(r.text)) {
    const title = /<title>([^<]*)<\/title>/i.exec(r.text)?.[1]?.trim();
    return `HTML page from ${r.server || "a proxy"}${title ? `: ${title}` : ""}`;
  }
  return r.text.slice(0, 160);
}

// ---------- results ----------

const rows = [];
const facts = {};

function record(step, command, r, note = "") {
  const row = {
    step,
    command,
    status: r.status,
    upMB: r.upBytes / MiB,
    downMB: r.downBytes / MiB,
    uploadS: r.uploadS,
    waitS: r.waitS,
    totalS: r.totalS,
    note,
  };
  const flags = [];
  if (r.totalS > SLOW_S) flags.push(`over ${SLOW_S} s (I1)`);
  if (r.status === 524) flags.push("524 Cloudflare timeout (I1)");
  if (r.status === 413) flags.push("413 too large (I2)");
  if (r.upBytes > BIG_MB * MiB) flags.push(`body over ${BIG_MB} MB (I2)`);
  if (r.status >= 400) flags.push(reason(r));
  row.flags = flags.join("; ");
  rows.push(row);
  const ok = r.status < 400 ? "ok" : `FAILED ${r.status}`;
  console.log(`  ${command}: ${ok} in ${r.totalS.toFixed(1)} s${row.flags ? ` — ${row.flags}` : ""}`);
  return row;
}

let token;

/** Runs a command; returns its result, or null when it failed (already recorded). */
async function rpc(step, command, args = {}, note = "") {
  let r;
  try {
    r = await send("POST", `/api/rpc/${command}`, { token, json: args });
  } catch (e) {
    rows.push({ step, command, status: 0, upMB: 0, downMB: 0, uploadS: 0, waitS: 0, totalS: 0, note, flags: `no answer: ${e.code ?? e.message}` });
    console.log(`  ${command}: no answer (${e.code ?? e.message})`);
    return null;
  }
  record(step, command, r, note);
  return r.status < 400 ? r.data : null;
}

/** Same, but quiet and not in the table (lists and cleanup). */
async function quiet(command, args = {}) {
  try {
    const r = await send("POST", `/api/rpc/${command}`, { token, json: args });
    if (r.status >= 400) {
      console.log(`  (${command}: ${r.status} ${reason(r)})`);
      return null;
    }
    return r.data;
  } catch (e) {
    console.log(`  (${command}: no answer, ${e.code ?? e.message})`);
    return null;
  }
}

function pad(s, n, right = false) {
  s = String(s);
  return right ? s.padStart(n) : s.padEnd(n);
}

function printTable() {
  console.log("\nResults (seconds; 'wait' = from the last byte sent to the first byte back, i.e. server + proxies)\n");
  const head = `${pad("step", 14)} ${pad("command", 24)} ${pad("status", 6, true)} ${pad("up MB", 8, true)} ${pad("down MB", 8, true)} ${pad("upload", 7, true)} ${pad("wait", 7, true)} ${pad("total", 7, true)}  notes`;
  console.log(head);
  console.log("-".repeat(head.length + 10));
  for (const r of rows) {
    console.log(
      `${pad(r.step, 14)} ${pad(r.command, 24)} ${pad(r.status, 6, true)} ${pad(r.upMB.toFixed(1), 8, true)} ${pad(r.downMB.toFixed(1), 8, true)} ${pad(r.uploadS.toFixed(1), 7, true)} ${pad(r.waitS.toFixed(1), 7, true)} ${pad(r.totalS.toFixed(1), 7, true)}  ${[r.note, r.flags].filter(Boolean).join("; ")}`,
    );
  }
  console.log("");
  for (const [k, v] of Object.entries(facts)) console.log(`${k}: ${v}`);
  const slow = rows.filter((r) => r.totalS > SLOW_S || r.status === 524);
  const big = rows.filter((r) => r.status === 413 || r.upMB > BIG_MB);
  console.log(`\nI1 (background jobs) trigger: ${slow.length ? `YES — ${slow.map((r) => r.command).join(", ")}` : `no step over ${SLOW_S} s`}`);
  console.log(`I2 (chunked upload) trigger: ${big.length ? `YES — ${big.map((r) => r.command).join(", ")}` : `no body over ${BIG_MB} MB`}`);
  const exportMb = facts["whole-library export (file)"];
  if (exportMb) console.log(`   (the export is ${exportMb}; as a base64 upload it is about 4/3 of that)`);
}

// ---------- the run ----------

const ids = (list) => new Set((list ?? []).map((x) => x.id ?? x.url));

async function snapshot() {
  return {
    packs: ids(await quiet("list_packs")),
    indexes: ids(await quiet("list_indexes")),
    fiveUrls: ids(await quiet("list_5etools_urls")),
    characters: ids(await quiet("list_characters")),
  };
}

/**
 * After update_indexes failed at a proxy (a 524 at 100 s), the server goes on with it. Wait until every index has been
 * checked (or 15 minutes), so the later steps and the cleanup do not race it, and record when it really finished.
 */
async function waitForIndexes(started, checked) {
  const last = rows[rows.length - 1];
  if (!last || (last.status !== 0 && last.status < 500)) return;
  console.log("  the server may still be updating; checking every 10 s ...");
  const deadline = started + 15 * 60 * 1000;
  while (performance.now() < deadline) {
    await new Promise((r) => setTimeout(r, 10_000));
    const list = await quiet("list_indexes");
    if (list && list.length && list.every((i) => i.lastChecked && i.lastChecked !== checked.get(i.url))) {
      const s = (performance.now() - started) / 1000;
      facts["update_indexes finished on the server after"] = `about ${s.toFixed(0)} s (the answer was ${last.status || "lost"} after ${last.totalS.toFixed(0)} s)`;
      console.log(`  finished on the server after about ${s.toFixed(0)} s`);
      return;
    }
  }
  facts["update_indexes finished on the server after"] = "not within 15 minutes";
}

async function cleanUp(before, label) {
  const now = await snapshot();
  const extra = (k) => [...now[k]].filter((id) => !before[k].has(id));
  const counts = [];
  for (const url of extra("fiveUrls")) await quiet("remove_5etools_url", { url });
  for (const url of extra("indexes")) await quiet("remove_index", { url });
  for (const packId of extra("packs")) await quiet("remove_pack", { packId });
  for (const characterId of extra("characters")) await quiet("delete_character", { characterId });
  counts.push(`${extra("packs").length} pack(s)`, `${extra("indexes").length} index(es)`, `${extra("fiveUrls").length} 5etools address(es)`, `${extra("characters").length} character(s)`);
  const after = await snapshot();
  const left = ["packs", "indexes", "fiveUrls", "characters"].reduce((n, k) => n + [...after[k]].filter((id) => !before[k].has(id)).length, 0);
  console.log(`  cleanup after ${label}: removed ${counts.join(", ")}${left ? ` — ${left} item(s) could not be removed` : ""}`);
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  target = { url: new URL(opts.server + "/") };
  if (opts.resolve) target.ip = opts.resolve;
  else if (opts.viaCloudflare) target.ip = await dohLookup(target.url.hostname);

  console.log(`Server ${target.url.origin}${target.ip ? ` via ${target.ip}` : ""}`);
  let info = null;
  const ttfb = [];
  for (let i = 0; i < 3; i++) {
    const r = await send("GET", "/api/server");
    ttfb.push(r.waitS);
    info = r.data;
    if (i === 0) facts["connected to"] = `${r.remote} (server header: ${r.server || "none"})`;
  }
  facts["server version"] = info?.version ?? "unknown";
  facts["/api/server time to first byte"] = ttfb.map((s) => `${(s * 1000).toFixed(0)} ms`).join(", ");

  const { user, password } = await credentials(opts);
  const login = await send("POST", "/api/auth/login", { json: { username: user, password } });
  if (login.status >= 400 || !login.data?.token) fail(`sign-in refused (${login.status}): ${reason(login)}`);
  token = login.data.token;
  console.log("Signed in.\n");

  const before = await snapshot();
  if (before.packs.size || before.indexes.size || before.characters.size) {
    console.log(`Note: the account already has ${before.packs.size} pack(s), ${before.indexes.size} index(es), ${before.characters.size} character(s); they are left alone.`);
  }

  try {
    // 1. Indexes.
    if (opts.index.length) {
      console.log("Indexes");
      for (const url of opts.index) await rpc("indexes", "add_index", { url }, basename(new URL(url).pathname));
      const checked = new Map(((await quiet("list_indexes")) ?? []).map((i) => [i.url, i.lastChecked]));
      const started = performance.now();
      const res = await rpc("indexes", "update_indexes", {}, `${opts.index.length} index(es), first download`);
      if (!res) await waitForIndexes(started, checked);
      if (res) facts["update_indexes"] = `updated ${res.updated}, failed ${res.failed?.length ?? 0}${res.failed?.length ? ` (${res.failed.join(", ")})` : ""}`;
      if (opts.repeatUpdate) {
        const again = await rpc("indexes", "update_indexes", {}, "again, nothing changed");
        if (again) facts["update_indexes (again)"] = `updated ${again.updated}, failed ${again.failed?.length ?? 0}`;
      }
    }

    // 2. 5etools.
    if (opts.fiveUrl) {
      console.log("5etools");
      const staged = await rpc("5etools", "stage_5etools_url", { url: opts.fiveUrl });
      if (staged) {
        const scan = await rpc("5etools", "scan_5etools_folder", { folder: staged.token });
        const all = (scan?.sources ?? []).map((s) => s.abbr);
        const sources = opts.fiveSources ?? all;
        if (sources.length) {
          const rep = await rpc("5etools", "import_5etools", { request: { folder: staged.token, sources, includeFluff: !!opts.fluff } }, `${sources.length} of ${all.length} source(s)`);
          if (rep) facts["import_5etools"] = `converted ${rep.converted}, replaced ${rep.replaced}, packs ${rep.packs?.length ?? 0}, skipped ${rep.skipped?.length ?? 0}`;
        }
      }
    }

    // 3. Aurora zip.
    if (opts.auroraZip) {
      console.log("Aurora zip");
      const bytes = readFileSync(opts.auroraZip);
      facts["Aurora zip"] = `${(bytes.length / MiB).toFixed(1)} MB (${basename(opts.auroraZip)})`;
      const preview = await rpc("aurora-zip", "preview_import_pack", { request: { files: [{ fileName: basename(opts.auroraZip), contentBase64: bytes.toString("base64") }] } }, `${(bytes.length / MiB).toFixed(1)} MB zip`);
      if (preview) {
        const res = await rpc("aurora-zip", "install_import_preview", { previewId: preview.previewId }, `${preview.converted} element(s)`);
        if (!res) await quiet("cancel_import_preview", { previewId: preview.previewId });
      }
    }

    // 4. Whole-library export.
    let exported = null;
    if (opts.export) {
      console.log("Export");
      const res = await rpc("export", "bundle_export", { selection: { characterIds: [], packs: "all" } });
      if (res) {
        exported = { name: res.fileName, bytes: Buffer.from(res.dataBase64, "base64") };
        facts["whole-library export (file)"] = `${(exported.bytes.length / MiB).toFixed(1)} MB, ${res.packs} pack(s), ${res.characters} character(s)`;
        if (opts.saveExport) {
          writeFileSync(opts.saveExport, exported.bytes);
          console.log(`  saved to ${opts.saveExport}`);
        }
      }
    }

    if (!opts.keep) await cleanUp(before, "imports");

    // 5. Bundle accept, into the cleaned library.
    if (opts.useBundle && (opts.bundle || exported)) {
      console.log("Bundle");
      const file = opts.bundle ? { name: basename(opts.bundle), bytes: readFileSync(opts.bundle) } : exported;
      facts["bundle"] = `${(file.bytes.length / MiB).toFixed(1)} MB (${opts.bundle ? file.name : "the export above"})`;
      const before2 = await snapshot();
      const staged = await rpc("bundle", "stage_bundle", { fileName: file.name, contentBase64: file.bytes.toString("base64") }, `${(file.bytes.length / MiB).toFixed(1)} MB file`);
      if (staged) {
        const opened = await rpc("bundle", "bundle_open", { token: staged.token }, "");
        if (opened) {
          const items = opened.preview?.items ?? [];
          const res = await rpc("bundle", "bundle_accept", { bundleId: opened.bundleId }, `${items.length} item(s)`);
          if (res) facts["bundle_accept"] = JSON.stringify(res);
          else await quiet("bundle_discard", { bundleId: opened.bundleId });
        }
      }
      if (!opts.keep) await cleanUp(before2, "bundle");
    }
  } finally {
    await send("POST", "/api/auth/logout", { token, json: {} }).catch(() => {});
    token = null;
  }

  printTable();
  if (opts.json) {
    writeFileSync(opts.json, JSON.stringify({ server: target.url.origin, via: target.ip ?? null, at: new Date().toISOString(), facts, rows }, null, 2));
    console.log(`\nWrote ${opts.json}`);
  }
}

main().catch((e) => {
  console.error(`measure-imports: ${e.stack ?? e}`);
  process.exit(1);
});
