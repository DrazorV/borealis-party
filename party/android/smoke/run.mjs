// The Android "Open with" smoke run (plan M7b, I4). Started by ../smoke-open-with.ps1; see party/README.md.
//
// On a running emulator (or the AVD it starts): a throwaway party server in WSL (server.sh, SQLite in /tmp, never the
// production server), the debug APK installed fresh, a sign-in through the app's own sign-in page, and then each test
// file opened with Borealis Party the way the Files app does it (an ACTION_VIEW of a content:// URI). For each one it
// checks the app dialog "Import <file> into smoke?" and what the import did on the server. The page is driven over
// the WebView's DevTools socket (debug APKs only). Screenshots go to the output folder.
//
// node party/android/smoke/run.mjs --apk <path> [--out <folder>] [--port 18799] [--avd BalanceLab_Tablet]
//   [--server-bin <WSL path>] [--keep]
// Exit code 0 when every check passed.

import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const PACKAGE = "app.borealis.party";
const ACTIVITY = `${PACKAGE}/app.borealis.MainActivity`;
const USER = "smoke";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((out, a, i, all) => {
    if (a.startsWith("--")) out.push([a.slice(2), all[i + 1] && !all[i + 1].startsWith("--") ? all[i + 1] : "true"]);
    return out;
  }, []),
);
const port = Number(args.port ?? 18799);
const avd = args.avd ?? "BalanceLab_Tablet";
const out = resolve(args.out ?? join(repo, "target", "party-android-smoke"));
const apk = args.apk ? resolve(args.apk) : null;
const serverBin = args["server-bin"] ?? process.env.BOREALIS_SERVER_WSL_BIN ?? "";
const devtoolsPort = 9333;
const password = randomBytes(12).toString("base64url");
mkdirSync(out, { recursive: true });

const sdk = process.env.ANDROID_HOME || process.env.ANDROID_SDK_ROOT || join(process.env.LOCALAPPDATA ?? "", "Android", "Sdk");
const adbPath = join(sdk, "platform-tools", process.platform === "win32" ? "adb.exe" : "adb");
const emulatorPath = join(sdk, "emulator", process.platform === "win32" ? "emulator.exe" : "emulator");

const results = [];
const log = (line) => console.log(line);
function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `: ${detail}` : ""}`);
  return ok;
}

function run(cmd, argv, opts = {}) {
  const r = spawnSync(cmd, argv, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, ...opts });
  if (r.error) throw r.error;
  if (r.status !== 0 && !opts.allowFail) throw new Error(`${cmd} ${argv.join(" ")} failed (${r.status}): ${r.stderr || r.stdout}`);
  return (r.stdout ?? "").trim();
}
const adb = (...a) => run(adbPath, a);
const adbShell = (cmd) => run(adbPath, ["shell", cmd]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, test, timeoutMs = 30000, every = 500) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const v = await test();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(every);
  }
}

// ---- emulator -------------------------------------------------------------------------------------------------------

async function ensureDevice() {
  const devices = adb("devices").split("\n").slice(1).filter((l) => /\tdevice$/.test(l));
  if (devices.length === 0) {
    log(`starting the emulator ${avd}`);
    spawn(emulatorPath, ["-avd", avd, "-no-snapshot-save", "-no-boot-anim"], { detached: true, stdio: "ignore" }).unref();
    adb("wait-for-device");
  }
  await until("the emulator to boot", () => adbShell("getprop sys.boot_completed") === "1", 240000, 2000);
  // 1080x2340 at 450 dpi is 384x832 CSS pixels, the phone the party UI is tested on (M7).
  adbShell("wm size 1080x2340");
  adbShell("wm density 450");
}

// ---- server ---------------------------------------------------------------------------------------------------------

const wslRepo = () => run("wsl.exe", ["-e", "wslpath", "-a", repo.replace(/\\/g, "/")]);

function startServer() {
  const bin = serverBin || run("wsl.exe", ["-e", "bash", "-lc", "echo $HOME/bp-accounts-target/release/borealis-server"]);
  const script = `${wslRepo()}/party/android/smoke/server.sh`;
  log(run("wsl.exe", ["-e", "bash", script, "start", bin, String(port), wslRepo()], { input: `${password}\n` }));
}
function stopServer() {
  log(run("wsl.exe", ["-e", "bash", `${wslRepo()}/party/android/smoke/server.sh`, "stop", String(port)], { allowFail: true }));
}

/** The server as the host sees it (WSL forwards its ports to localhost). */
async function api(token, command, body = {}) {
  const r = await fetch(`http://127.0.0.1:${port}/api/rpc/${command}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify(body),
  });
  const text = await r.text();
  if (!r.ok) throw new Error(`${command}: ${r.status} ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}

// ---- the page, over DevTools ----------------------------------------------------------------------------------------

let ws;
let nextId = 1;
const pending = new Map();

async function connectPage() {
  const socket = await until("the WebView's DevTools socket", () => {
    const pid = adbShell(`pidof ${PACKAGE}`).split(/\s+/)[0];
    if (!pid) return null;
    return adbShell("cat /proc/net/unix").includes(`webview_devtools_remote_${pid}`) ? `webview_devtools_remote_${pid}` : null;
  }, 60000);
  adb("forward", `tcp:${devtoolsPort}`, `localabstract:${socket}`);
  const target = await until("the app's page", async () => {
    try {
      const list = await (await fetch(`http://127.0.0.1:${devtoolsPort}/json`)).json();
      return list.find((t) => t.type === "page" && /tauri\.localhost/.test(t.url));
    } catch { return null; }
  }, 60000);
  ws?.close();
  ws = new WebSocket(target.webSocketDebuggerUrl);
  ws.onmessage = (m) => {
    const msg = JSON.parse(String(m.data));
    const p = pending.get(msg.id);
    if (p) { pending.delete(msg.id); msg.error ? p.reject(new Error(msg.error.message)) : p.resolve(msg.result); }
  };
  await new Promise((ok, bad) => { ws.onopen = ok; ws.onerror = bad; });
}

function cdp(method, params = {}) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((resolveIt, reject) => pending.set(id, { resolve: resolveIt, reject }));
}

/** Evaluates `expression` in the page (awaiting a promise) and answers its JSON value. */
async function page(expression) {
  const r = await cdp("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
}

async function screenshot(name) {
  const r = await cdp("Page.captureScreenshot", { format: "png" });
  writeFileSync(join(out, `${name}.png`), Buffer.from(r.data, "base64"));
}

/** Helpers put into the page once per load. */
const HELPERS = `(() => {
  if (window.__smoke) return true;
  const visible = (el) => !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
  const setValue = (input, value) => {
    const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  };
  window.__smoke = {
    dialogs: () => [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')].filter(visible).map((d) => ({
      text: d.innerText, buttons: [...d.querySelectorAll("button")].filter(visible).map((b) => b.innerText.trim()).filter(Boolean),
    })),
    click: (label, inDialog) => {
      const scope = inDialog ? [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')].filter(visible).pop() : document;
      const b = scope && [...scope.querySelectorAll("button")].filter(visible).find((x) => x.innerText.trim() === label);
      if (!b || b.disabled) return false;
      b.click();
      return true;
    },
    clickIn: (part, label) => {
      const d = [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')].filter(visible).find((x) => x.innerText.includes(part));
      const b = d && [...d.querySelectorAll("button")].filter(visible).find((x) => x.innerText.trim() === label);
      if (!b || b.disabled) return false;
      b.click();
      return true;
    },
    fill: (selector, value) => { const i = document.querySelector(selector); if (!i) return false; setValue(i, value); return true; },
    text: () => document.body.innerText,
  };
  return true;
})()`;
const smoke = async (call) => { await page(HELPERS); return page(`window.__smoke.${call}`); };

// ---- opening files the way the Files app does -----------------------------------------------------------------------

/** Puts `file` in Downloads and answers its MediaStore content URI. */
async function pushToDownloads(file, name) {
  adb("push", file, `/sdcard/Download/${name}`);
  adbShell(`am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE -d file:///sdcard/Download/${name}`);
  const id = await until(`${name} in MediaStore`, () => {
    const row = adbShell(`content query --uri content://media/external/downloads --projection _id:_display_name --where "_display_name='${name}'"`);
    return /_id=(\d+)/.exec(row)?.[1];
  }, 20000);
  return `content://media/external/downloads/${id}`;
}

function openWith(uri, mime) {
  adbShell(`am start -W -a android.intent.action.VIEW -d '${uri}' -t '${mime}' --grant-read-uri-permission -n ${ACTIVITY}`);
}

const askFor = (name) => `Import ${name} into ${USER}?`;

async function waitForAsk(name) {
  return until(`the import question for ${name}`, async () => (await smoke("dialogs()")).find((d) => d.text.includes(askFor(name))), 20000);
}

/** Answers the import question for `name` with `label`, until that dialog has closed. */
async function answerAsk(name, label) {
  await until(`the answer ${label} to the question for ${name}`, async () => {
    const open = (await smoke("dialogs()")).some((d) => d.text.includes(askFor(name)));
    if (!open) return true;
    await smoke(`clickIn(${JSON.stringify(askFor(name))}, ${JSON.stringify(label)})`);
    return false;
  }, 15000, 700);
}

/** Clicks through dialogs whose buttons include one of `labels` until `done` answers true. */
async function clickThrough(labels, done, timeoutMs = 60000) {
  const seen = [];
  await until("the import to finish", async () => {
    if (await done()) return true;
    for (const d of await smoke("dialogs()")) {
      const label = labels.find((l) => d.buttons.includes(l));
      if (label && await smoke(`click(${JSON.stringify(label)}, true)`)) { seen.push(label); await sleep(800); break; }
    }
    return false;
  }, timeoutMs, 1000);
  return seen;
}

// ---- the run --------------------------------------------------------------------------------------------------------

async function main() {
  if (!apk || !existsSync(apk)) throw new Error(`no APK at ${apk}; build one with party/android/build-apk.ps1 -Target x86_64 -Debug`);
  await ensureDevice();
  startServer();

  adb("install", "-r", "-t", apk);
  adbShell(`pm clear ${PACKAGE}`);
  adbShell(`am start -W -n ${ACTIVITY}`);
  await connectPage();

  // Sign in through the app's own pages: the server's address, then the account.
  await until("the sign-in page", async () => (await smoke("fill('input[inputmode=url]', 'http://10.0.2.2:" + port + "')")), 60000);
  await smoke("click('Connect')");
  await until("the account form", () => smoke("fill('input[autocomplete=username]', " + JSON.stringify(USER) + ")"), 30000);
  await smoke("fill('input[type=password]', " + JSON.stringify(password) + ")");
  await screenshot("01-sign-in");
  await smoke("click('Sign in')");
  // Signing in reloads the page.
  await until("the signed-in page", async () => {
    try { await connectPage(); return await page("!!localStorage.getItem('party.session') && !document.querySelector('input[type=password]')"); } catch { return false; }
  }, 60000, 2000);
  const token = await page("JSON.parse(localStorage.getItem('party.session')).token");
  check("signs in to the throwaway server", !!token);
  await sleep(3000);
  // Upstream's first-start notices, if any, are closed so they don't cover the import dialogs.
  for (const label of ["Close", "Dismiss", "OK", "Got it"]) await smoke(`click(${JSON.stringify(label)}, true)`).catch(() => false);
  await screenshot("02-signed-in");

  const characters = async () => (await api(token, "list_characters")).map((c) => c.name);
  const packs = async () => (await api(token, "list_packs")).map((p) => p.name ?? p.id);

  // 1. A character file: asked first, then imported into this account on the server.
  const before = (await characters()).length;
  const hero = await pushToDownloads(join(repo, "e2e/fixtures/d5-hero.dnd5e"), "Smoke-Hero.dnd5e");
  openWith(hero, "application/octet-stream");
  await waitForAsk("Smoke-Hero.dnd5e");
  check("a .dnd5e asks first: \"Import Smoke-Hero.dnd5e into smoke?\"", true);
  await screenshot("03-ask-character");
  await answerAsk("Smoke-Hero.dnd5e", "Import");
  const added = await until("the character on the server", async () => ((await characters()).length > before ? await characters() : null), 30000).catch(() => null);
  check(".dnd5e imported into the account", !!added, added ? added.join(", ") : "nothing on the server");
  await sleep(1500);
  await screenshot("04-character-imported");

  // 2. The same file again, and Cancel: nothing changes.
  const count = (await characters()).length;
  openWith(hero, "application/octet-stream");
  await waitForAsk("Smoke-Hero.dnd5e");
  await answerAsk("Smoke-Hero.dnd5e", "Cancel");
  await sleep(4000);
  check("Cancel imports nothing", (await characters()).length === count);

  // 3. An Aurora XML: asked first, then upstream's import preview, installed into the account's content.
  const xml = await pushToDownloads(join(here, "smoke-feats.xml"), "smoke-feats.xml");
  openWith(xml, "text/xml");
  await waitForAsk("smoke-feats.xml");
  check("an .xml asks first", true);
  await answerAsk("smoke-feats.xml", "Import");
  await sleep(1500);
  await screenshot("05-xml-after-ask");
  const clicked = await clickThrough(["I understand, continue", "Install", "Done", "OK", "Close"], async () => (await packs()).some((p) => /smoke/i.test(String(p))));
  check("the .xml went through the import preview into the account's content", (await packs()).some((p) => /smoke/i.test(String(p))), `clicked ${clicked.join(" > ") || "nothing"}`);
  await screenshot("06-xml-imported");
  await until("the import report to close", async () => !(await smoke("dialogs()")).some((d) => d.text.startsWith("Import report")) || (await smoke("clickIn('Import report', 'Close')"), false), 15000, 700);

  // 4. 5etools-format homebrew (.json): asked first, then handed to Additional Content's 5etools import.
  const json = await pushToDownloads(join(here, "smoke-homebrew.json"), "smoke-homebrew.json");
  openWith(json, "application/json");
  await waitForAsk("smoke-homebrew.json");
  check("a 5etools .json asks first (the party manifest adds application/json)", true);
  await answerAsk("smoke-homebrew.json", "Import");
  const routed = await until("the 5etools import", async () => /smoke-homebrew\.json/.test(await smoke("text()")), 20000).catch(() => false);
  check("the .json goes to the 5etools-format import", !!routed);
  await screenshot("07-json-routed");

  // 5. A .borealis made by the server from the imported character: Android hands it over with the other files
  //    (`take_opened_files`, no bundle queue without device-link), so it opens upstream's Open bundle window.
  const list = await api(token, "list_characters");
  const bundle = await api(token, "bundle_export", { selection: { characterIds: [list[0].id], packs: "none" } });
  const bundleFile = join(out, "Smoke-Party.borealis");
  writeFileSync(bundleFile, Buffer.from(bundle.dataBase64, "base64"));
  const borealis = await pushToDownloads(bundleFile, "Smoke-Party.borealis");
  openWith(borealis, "application/octet-stream");
  const asked = await waitForAsk("Smoke-Party.borealis").catch(() => null);
  check("a .borealis reaches the page (the files queue) and asks first", !!asked);
  if (asked) {
    await answerAsk("Smoke-Party.borealis", "Import");
    const shown = await until("the Open bundle window", async () => (await smoke("dialogs()")).find((d) => d.text.startsWith("Open bundle") && d.text.includes("Smoke-Party.borealis")), 30000).catch(() => null);
    check("the .borealis opens the Open bundle window (staged on the server)", !!shown, shown ? shown.text.split(/\n+/).slice(1, 3).join(" / ") : "");
    await screenshot("08-bundle-window");
    await smoke("clickIn('Open bundle', 'Cancel')");
  }

  // 6. M5: a tap on a link to a party page (here a link put on the page) goes to the system browser through the
  //    device's vault_open_url, or, where it can't, to the app dialog with Copy link.
  await sleep(1000);
  await page(`(() => { const a = document.createElement("a"); a.href = "https://example.org/borealis-party-smoke"; a.textContent = "smoke link"; a.id = "smoke-link"; document.body.appendChild(a); a.click(); a.remove(); return true; })()`);
  const opened = await until("the browser or the copy dialog", async () => {
    const top = adbShell("dumpsys activity activities").split("\n").find((l) => /topResumedActivity|mResumedActivity/.test(l)) ?? "";
    if (top && !top.includes(PACKAGE)) return `browser: ${top.trim()}`;
    const copy = (await smoke("dialogs()")).find((d) => d.text.startsWith("Open in your browser"));
    return copy ? "copy dialog" : null;
  }, 20000, 1000).catch(() => null);
  check("a party link opens outside the app (M5)", !!opened, opened ?? "nothing happened");
  // Back to the app (without -W: bringing a running task forward never reports a launch, so -W would wait for ever).
  adbShell(`am start -n ${ACTIVITY}`);
}

let failed = false;
try {
  await main();
} catch (e) {
  failed = true;
  check("the run finished", false, String(e?.message ?? e));
  try { await screenshot("99-failure"); } catch { /* no page */ }
} finally {
  ws?.close();
  if (!args.keep) {
    stopServer();
    for (const name of ["Smoke-Hero.dnd5e", "smoke-feats.xml", "smoke-homebrew.json", "Smoke-Party.borealis"]) {
      run(adbPath, ["shell", `rm -f /sdcard/Download/${name}`], { allowFail: true });
    }
    run(adbPath, ["shell", `pm clear ${PACKAGE}`], { allowFail: true });
    run(adbPath, ["shell", "wm size reset"], { allowFail: true });
    run(adbPath, ["shell", "wm density reset"], { allowFail: true });
    run(adbPath, ["forward", "--remove", `tcp:${devtoolsPort}`], { allowFail: true });
  }
  const bad = results.filter((r) => !r.ok);
  writeFileSync(join(out, "results.json"), JSON.stringify(results, null, 2));
  log(`\n${results.length - bad.length}/${results.length} checks passed. Screenshots and results.json in ${out}`);
  process.exit(failed || bad.length ? 1 : 0);
}
