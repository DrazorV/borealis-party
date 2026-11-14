// The Android "Open with" smoke run (plan M7b, I4). Started by ../smoke-open-with.ps1; see party/README.md.
//
// On a running emulator (or the AVD it starts): a throwaway party server in WSL (server.sh, SQLite in /tmp, never the
// production server), the debug APK installed fresh, a sign-in through the app's own sign-in page, and then each test
// file opened with Borealis Party the way the Files app does it (an ACTION_VIEW of a content:// URI). For each one it
// checks the app dialog "Import <file> into smoke?" and what the import did on the server. The page is driven over
// the WebView's DevTools socket (debug APKs only). Screenshots go to the output folder.
// It also opens the imported character's sheet preview at Android font sizes 1.0 and 1.15 (sheetPreview below).
//
// node party/android/smoke/run.mjs --apk <path> [--out <folder>] [--port 18799] [--avd BalanceLab_Tablet]
//   [--server-bin <WSL path>] [--keep] [--no-wide] [--sheet-only] [--combat-only] [--max-minutes 20]
// --sheet-only: sign in, import the character and check its sheet preview (step 7), nothing else.
// --combat-only: sign in, import the character, and check the DM's combat tracker (combatTracker) and the player's
// Combat sub-tab (playerCombat), nothing else.
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
const sheetOnly = args["sheet-only"] === "true";
const combatOnly = args["combat-only"] === "true";
// The whole run stops after this many minutes (default 20), so a stalled emulator never holds it for hours.
const maxMinutes = Number(args["max-minutes"] ?? 20);
if (!Number.isFinite(maxMinutes) || maxMinutes <= 0) throw new Error(`--max-minutes needs a number of minutes above 0, not ${args["max-minutes"]}`);
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

/** One command, at most `opts.timeoutMs` (2 min by default): a stuck adb or WSL call fails the run instead of hanging it. */
function run(cmd, argv, opts = {}) {
  const { timeoutMs = 120000, ...rest } = opts;
  const r = spawnSync(cmd, argv, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout: timeoutMs, killSignal: "SIGKILL", ...rest });
  if (r.error?.code === "ETIMEDOUT") {
    if (opts.allowFail) return "";
    throw new Error(`${cmd} ${argv.join(" ")}: no answer in ${timeoutMs / 1000} s`);
  }
  if (r.error) { if (opts.allowFail) return ""; throw r.error; }
  if (r.status !== 0 && !opts.allowFail) throw new Error(`${cmd} ${argv.join(" ")} failed (${r.status}): ${r.stderr || r.stdout}`);
  return (r.stdout ?? "").trim();
}
const adb = (...a) => run(adbPath, a, { timeoutMs: a[0] === "install" || a[0] === "wait-for-device" ? 300000 : 120000 });
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
  await sleep(2000);
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

/** One DevTools call; it fails after `timeoutMs` (a page that went away, as after a screen-size change, never answers). */
function cdp(method, params = {}, timeoutMs = 60000) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((resolveIt, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method}: no answer in ${timeoutMs / 1000} s`)); }, timeoutMs);
    pending.set(id, { resolve: (v) => { clearTimeout(timer); resolveIt(v); }, reject: (e) => { clearTimeout(timer); reject(e); } });
  });
}

/** After a screen-size change Android may recreate the activity and its WebView: connect to the page again. */
async function resized(what, fits) {
  await sleep(3000);
  await until(what, async () => {
    try { await connectPage(); return fits(await cdp("Runtime.evaluate", { expression: "innerWidth", returnByValue: true }, 5000).then((r) => r.result.value)); } catch { return false; }
  }, 60000, 2000);
}

/** Evaluates `expression` in the page (awaiting a promise) and answers its JSON value. */
async function page(expression) {
  const r = await cdp("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
}

/**
 * A screenshot for the output folder. A web view that is not in front draws no frames, so its screenshot never
 * answers (seen after the system browser opened a party link): the app is brought to the front, the page reconnected
 * and the screenshot tried once more. A screenshot is evidence, not a check: if it still fails, the run goes on.
 */
async function screenshot(name) {
  for (const attempt of [1, 2]) {
    try {
      const r = await cdp("Page.captureScreenshot", { format: "png" }, attempt === 1 ? 60000 : 30000);
      writeFileSync(join(out, `${name}.png`), Buffer.from(r.data, "base64"));
      return;
    } catch (e) {
      log(`      screenshot ${name}: ${e?.message ?? e}${attempt === 1 ? "; bringing the app to the front and trying once more" : "; skipped"}`);
      if (attempt === 1) {
        run(adbPath, ["shell", `am start -n ${ACTIVITY}`], { allowFail: true });
        await sleep(2000);
        await connectPage().catch(() => null);
      }
    }
  }
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
  // A screen-size change just before the start can recreate the activity once: reconnect and try again.
  await until("the sign-in page", async () => {
    try { return await smoke("fill('input[inputmode=url]', 'http://10.0.2.2:" + port + "')"); } catch { await connectPage().catch(() => null); return false; }
  }, 60000, 1500);
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
  if (!sheetOnly && !combatOnly) await phoneLayout();

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
  if (sheetOnly) { await sheetPreview(token); return; }
  if (combatOnly) { await combatTracker(); await playerCombat(token); return; }

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
  // One import page (0.2.1): "Open with" lands on Additional Content's Import section, and there is no other.
  const where = await page(`({ hash: location.hash, sections: document.querySelectorAll("section#party-import").length,
    first: [...document.querySelectorAll(".v4-content section.v4-csec")].filter((s) => s.offsetParent !== null)[0]?.getAttribute("aria-label") ?? "",
    upstream: [...document.querySelectorAll(".v4-content button")].filter((b) => b.offsetParent !== null && b.textContent.trim() === "Import files").length })`);
  check("Open with lands on Additional Content's Import section (one import page)",
    where.hash === "#/content" && where.sections === 1 && where.first === "Import" && where.upstream === 0, JSON.stringify(where));
  // Its 5etools card: the account's own source, and the server's default only when its admin set one (this throwaway
  // server sets none).
  const sources = await until("the 5etools sources in the Import section", async () => {
    const s = await page(`[...document.querySelectorAll(".party-import-source h3")].map((h) => h.textContent.trim())`);
    return s.length ? s : null;
  }, 15000).catch(() => []);
  check("the Import section offers an own 5etools source (and no default the server didn't set)",
    sources.join("|") === "My own 5etools source", sources.join(" | "));

  // 5. A .borealis made by the server from the imported character: Android hands it over with the other files
  //    (`take_opened_files`, no bundle queue without device-link), so it opens upstream's Open bundle window, through
  //    the Import screen (and its content notice).
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
    // Since the unified imports the file goes through the Import screen, which shows upstream's content notice first
    // unless the account turned it off; continue past it.
    const shown = await until("the Open bundle window", async () => {
      const open = await smoke("dialogs()");
      if (open.some((d) => d.text.startsWith("Import only content you own"))) {
        await smoke("clickIn('Import only content you own', 'I understand, continue')");
        return null;
      }
      return open.find((d) => d.text.startsWith("Open bundle") && d.text.includes("Smoke-Party.borealis"));
    }, 30000).catch(() => null);
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
  await sleep(2000);
  await partyLive(token);
  await encounters(token);
  await combatTracker();
  await playerCombat(token);

  // 7. The sheet preview at Android font sizes 1.0 and 1.15 (last: it restarts the app).
  await sheetPreview(token);
}

/** Any other route of the server's API, as the host sees it. */
async function rest(token, method, path, body) {
  const r = await fetch(`http://127.0.0.1:${port}${path}`, {
    method,
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${text.slice(0, 200)}`);
  return text ? JSON.parse(text) : null;
}

/**
 * The live party roster in the real app (0.2.1): the account runs a party with its own character in it (D4), the
 * Parties page shows the character's numbers, and damage taken on the server shows there within a few seconds,
 * through the app's own event stream (the browser rig has no Tauri). View opens the read-only sheet, which fits the
 * phone's width.
 */
async function partyLive(token) {
  const hero = (await api(token, "list_characters"))[0]?.id;
  if (!hero) { check("the live party roster (needs the imported character)", false, "no character"); return; }
  await rest(token, "POST", "/api/me/roles/dm", {});
  const party = await rest(token, "POST", "/api/parties", { name: "Smoke Party" });
  try {
    await api(token, "load_character", { characterId: hero });
    await rest(token, "POST", `/api/parties/${party.id}/members`, { characterId: hero });
    await connectPage().catch(() => null);
    await page(`location.hash = "#/parties"`);
    const label = `(() => { const e = document.querySelector('.party-parties [aria-label^="Hit points "]'); return e ? e.getAttribute("aria-label") : null; })()`;
    const shown = await until("the roster's hit points", () => page(label), 30000).catch(() => null);
    check("the Parties page shows the member's numbers", !!shown, shown ?? "no hit points on the roster");
    if (!shown) return;
    const [, now, max] = /Hit points (\d+) of (\d+)/.exec(shown).map(Number);
    const started = Date.now();
    await api(token, "take_damage", { characterId: hero, amount: 1 });
    const want = `Hit points ${now - 1} of ${max}`;
    const followed = await until("the roster to follow the damage", async () => ((await page(label)) ?? "").startsWith(want), 5000, 200).catch(() => false);
    check("the roster follows damage live in the app (its event stream)", !!followed, followed ? `${Date.now() - started} ms` : `still ${await page(label)}`);
    await screenshot("09-party-roster");
    await page(`(() => { const a = document.querySelector('.party-parties a[aria-label^="View "]'); if (a) a.click(); return !!a; })()`);
    const sheet = await until("the read-only sheet", () => page(`document.body.innerText.includes("Read-only") && !!document.querySelector('[role="group"][aria-label^="Hit points "]')`), 30000).catch(() => false);
    check("View opens the DM's read-only sheet", !!sheet);
    const sideways = await page(`document.documentElement.scrollWidth - innerWidth`);
    check("the sheet fits the phone's width", sideways <= 0, `${sideways} px too wide`);
    await screenshot("10-party-sheet");
  } finally {
    await rest(token, "DELETE", `/api/parties/${party.id}`).catch(() => null);
    await page(`location.hash = "#/characters"`).catch(() => null);
  }
}

/**
 * The encounter builder in the real app (0.3.0): the account runs a party with an encounter of SRD monsters, the
 * party's Encounters list shows it, the builder opens with its summary bar on screen (no XP: D16), and neither page
 * scrolls sideways.
 */
async function encounters(token) {
  await rest(token, "POST", "/api/me/roles/dm", {});
  const party = await rest(token, "POST", "/api/parties", { name: "Smoke Encounters" });
  try {
    const enc = await rest(token, "POST", `/api/parties/${party.id}/encounters`, { name: "Smoke ambush" });
    await rest(token, "POST", `/api/encounters/${enc.id}/creatures`, { elementId: "ID_SRD521_MONSTER_GOBLIN_WARRIOR", count: 4 });
    await connectPage().catch(() => null);
    await page(`location.hash = "#/parties/${party.id}/encounters"`);
    const row = `(() => { const a = document.querySelector('a.enc-row[aria-label="Open Smoke ambush"]'); return a ? a.textContent : null; })()`;
    const listed = await until("the Encounters list", () => page(row), 30000).catch(() => null);
    check("the party's Encounters list shows the encounter", !!listed && listed.includes("4 creatures (4 hidden)"), listed ?? "no row");
    const wideList = await page(`document.documentElement.scrollWidth - innerWidth`);
    check("the Encounters list fits the phone's width", wideList <= 0, `${wideList} px too wide`);
    await screenshot("11-encounters-list");
    await page(`(() => { const a = document.querySelector('a.enc-row'); if (a) a.click(); return !!a; })()`);
    const bar = `(() => { const b = document.querySelector('.enc-bar'); if (!b) return null; const r = b.getBoundingClientRect(); return { text: b.textContent, bottom: r.bottom, height: innerHeight }; })()`;
    const shown = await until("the builder's summary bar", () => page(bar), 30000).catch(() => null);
    check("the builder counts the creatures, with no XP", !!shown && shown.text.includes("4 creatures") && !shown.text.includes("XP"), shown ? shown.text.slice(0, 80) : "no bar");
    check("the summary bar is on screen", !!shown && shown.bottom <= shown.height + 1, shown ? `bottom ${Math.round(shown.bottom)} of ${shown.height}` : "");
    const wideBuilder = await page(`document.documentElement.scrollWidth - innerWidth`);
    check("the builder fits the phone's width", wideBuilder <= 0, `${wideBuilder} px too wide`);
    await screenshot("12-encounter-builder");
  } finally {
    await rest(token, "DELETE", `/api/parties/${party.id}`).catch(() => null);
    await page(`location.hash = "#/characters"`).catch(() => null);
  }
}

/**
 * The DM's combat tracker in the real app (0.3.0): the browser rig has no Tauri, so this is where the app's own phone
 * shell is checked. It runs on the tracker's mock server (`party.combatMock`, an SRD sample fight), so it needs no
 * combat routes on the throwaway server. Checks: the tracker opens, Next turn sits above the app's tab bar (a
 * full-size target) and starts the fight, a typed damage changes HP, and nothing scrolls sideways. The Live chip is
 * only logged: it comes with the player view's branch.
 */
async function combatTracker() {
  const reloadTo = async (hash) => {
    await page(`location.hash = ${JSON.stringify(hash)}; location.reload(); true`).catch(() => null);
    await sleep(2000);
    await until("the page after a reload", async () => { try { await connectPage(); return await page("document.readyState === 'complete'"); } catch { return false; } }, 60000, 1500);
  };
  try {
    await page(`localStorage.setItem("party.combatMock", "1"); true`);
    await reloadTo("#/combat/c1");
    const opened = await until("the combat tracker", () => page(`document.querySelectorAll("[data-combatant]").length`), 30000).catch(() => 0);
    check("the DM's combat tracker opens in the app", opened > 0, `${opened} combatants`);
    if (!opened) return;
    // The Live chip is the player view's (feat/player-combat); with both merged it shows here for the DM too.
    const chip = await page(`(() => { const b = [...document.querySelectorAll(".party-bar button")].find((x) => /^\W*Live/.test(x.textContent.trim()) && x.getClientRects().length);
      return b ? b.textContent.trim() : null; })()`).catch(() => null);
    log(`      Live chip in the top bar: ${chip ?? "not shown (needs feat/player-combat, and the real combat routes)"}`);
    for (const [label, n] of [["Knight", 18], ["Adult Red Dragon", 12], ["Priest", 10], ["Cultist 1", 9], ["Cultist 2", 8], ["Mage", 5]]) {
      await page(`(() => { const i = document.querySelector('input[aria-label=${JSON.stringify(`${label} initiative`)}]'); if (!i) return false;
        const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, ${JSON.stringify(String(n))});
        i.dispatchEvent(new Event("input", { bubbles: true })); i.dispatchEvent(new FocusEvent("blur")); i.dispatchEvent(new FocusEvent("focusout", { bubbles: true })); return true; })()`);
      await sleep(400);
    }
    const thumb = await page(`(() => {
      const bar = document.querySelector(".cb-thumb"); const tabs = document.querySelector("nav.aur-bottombar");
      if (!bar || !bar.getClientRects().length) return null;
      const b = bar.getBoundingClientRect(); const t = tabs ? tabs.getBoundingClientRect() : null;
      return { bottom: Math.round(b.bottom), tabsTop: t ? Math.round(t.top) : null, h: innerHeight, button: bar.querySelector(".btn")?.getBoundingClientRect().height };
    })()`);
    check("Next turn sits in the thumb zone, above the app's tab bar", !!thumb && thumb.bottom <= (thumb.tabsTop ?? thumb.h) + 1 && thumb.bottom > thumb.h * 0.6,
      thumb ? JSON.stringify(thumb) : "no thumb bar");
    check("Next turn is a full-size tap target (48 px on Android)", !!thumb && thumb.button >= 47.5, thumb ? `${thumb.button}px` : "");
    await page(`document.querySelector(".cb-thumb .btn")?.click(); true`);
    const started = await until("round 1", () => page(`!!document.querySelector('[data-combatant][aria-current="true"]')`), 15000).catch(() => false);
    check("Start begins round 1 with the first combatant", !!started);
    await page(`document.querySelector('[data-combatant="mage"] .cb-row-main')?.click(); true`);
    await sleep(600);
    await page(`(() => { const i = document.querySelector('#cb-card input[aria-label^="Damage or healing"]'); if (!i) return false;
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; set.call(i, "7"); i.dispatchEvent(new Event("input", { bubbles: true }));
      [...document.querySelectorAll("#cb-card button")].find((b) => b.textContent.trim() === "Damage")?.click(); return true; })()`);
    const hp = await until("the mage's HP", () => page(`(() => { const t = document.querySelector('[data-combatant="mage"] .cb-hp-text'); return t && t.textContent.startsWith("33") ? t.textContent : null; })()`), 10000).catch(() => null);
    check("a typed damage changes the creature's HP", !!hp, hp ?? "unchanged");
    const sideways = await page(`document.documentElement.scrollWidth - innerWidth`);
    check("the tracker fits the phone's width", sideways <= 0, `${sideways} px too wide`);
    await screenshot("13-combat-tracker");
  } finally {
    await page(`localStorage.removeItem("party.combatMock"); true`).catch(() => null);
    await reloadTo("#/characters").catch(() => null);
  }
}

/**
 * The player's Combat sub-tab in the real app (0.3.0 part F): while the account's party has a live combat, the Live
 * chip shows in the top chrome, opens the Play tab's Combat sub-tab, and the DM's "Take 2 slashing?" prompt applied
 * there takes the damage on the player's own character on the server (D7 a); after the app goes to the background
 * and comes back, the view shows what changed meanwhile (no stale state). Until the combat server is in this tree,
 * the server's PLAYER projection is answered inside the WebView (a fetch wrapper put into the running page, which
 * then asks again as it does when the network comes back), so what is checked is the app side: the tab injection,
 * the chrome, the app's own fetch, the wake-up refresh and the player's own command.
 */
async function playerCombat(token) {
  const hero = (await api(token, "list_characters"))[0];
  if (!hero) { check("the Combat sub-tab (needs the imported character)", false, "no character"); return; }
  await rest(token, "POST", "/api/me/roles/dm", {});
  const party = await rest(token, "POST", "/api/parties", { name: "Smoke Fight" });
  let shimmed = false;
  try {
    await api(token, "load_character", { characterId: hero.id });
    await rest(token, "POST", `/api/parties/${party.id}/members`, { characterId: hero.id });
    const combat = {
      id: "smoke-combat", partyId: party.id, pseq: 1, status: "active", turn: { kind: "combatant", id: "cr1", round: 2 },
      encounter: { id: "e1", publicName: "Smoke ambush" },
      order: [
        { id: "cr1", kind: "creature", name: "Goblin", mine: false, initiative: 16, band: "bloodied", conditions: [{ name: "prone" }],
          statBlock: { size: ["small"], creature_type: "humanoid", ac: [{ value: 15 }], hp: { average: 7, formula: "2d6" }, actions: [{ name: "Scimitar", text: "Melee Weapon Attack: +4 to hit." }] } },
        { id: "me", kind: "character", name: hero.name, mine: true, initiative: 12, conditions: [], life: { state: "alive" } },
      ],
      me: [{ id: "me", kind: "character", characterId: hero.id, hp: { current: 10, max: 10, temp: 0 }, life: { state: "alive" }, concentrationChecks: [],
        prompts: [{ id: "pr1", kind: "damage", amount: 2, parts: [{ amount: 2, damage_type: "slashing" }], criticalHit: false }] }],
    };
    const shim = `(() => {
      const state = ${JSON.stringify(combat)};
      window.__smokeCombat = state;
      const real = window.__smokeRealFetch || window.fetch.bind(window);
      window.__smokeRealFetch = real;
      const json = (v) => new Response(JSON.stringify(v), { status: 200, headers: { "content-type": "application/json" } });
      window.fetch = (input, init) => {
        const url = String(typeof input === "string" ? input : input.url);
        // No regular expressions here: this is a template literal, where a backslash would be lost.
        const path = url.replace(/^[a-z]+:[/][/][^/]+/, "").split("?")[0];
        const parts = path.split("/");
        if (parts[1] === "api" && parts[2] === "parties" && parts[4] === "combat" && parts.length === 5) return Promise.resolve(json({ combat: state.id }));
        if (parts[1] === "api" && parts[2] === "combats" && parts.length === 4) return Promise.resolve(json(state));
        const m = parts[1] === "api" && parts[2] === "combats" && parts.length === 5 ? [path, parts[4]] : null;
        if (m) {
          const body = JSON.parse((init && init.body) || "{}");
          const args = body.args || {};
          if (m[1] === "resolve_prompt") for (const me of state.me) me.prompts = (me.prompts || []).filter((p) => p.id !== args.promptId);
          if (m[1] === "summon" && args.combatant) {
            const id = "smoke-s" + state.order.length;
            const hp = { current: args.combatant.hp.current, max: args.combatant.hp.max, temp: 0 };
            const at = state.order.findIndex((x) => x.id === args.owner);
            state.order.splice(at + 1, 0, { id, kind: "creature", name: args.combatant.public_name, mine: true, band: "unhurt", life: { state: "alive" },
              conditions: [], exhaustion: 0, view: { owner_id: args.owner, hp, ac: args.combatant.ac } });
            state.me.push({ id, kind: "creature", hp, life: { state: "alive" }, concentrationChecks: [], prompts: [] });
          }
          state.pseq += 1;
          return Promise.resolve(json(state));
        }
        return real(input, init);
      };
      return true;
    })()`;
    await connectPage().catch(() => null);
    await page(`location.hash = "#/characters"`);
    await page(shim);
    shimmed = true;
    // The page asks for its combats again when the network comes back (combat/player/store.ts).
    await page(`window.dispatchEvent(new Event("online")), true`);
    const chip = await until("the Live chip", () => page(`!!document.querySelector(".combat-live-chip")`), 30000).catch(() => false);
    check("a live combat shows the Live chip in the app's top chrome", !!chip, chip ? "" : await page(`JSON.stringify({ hash: location.hash, shim: String(window.fetch).includes("combat"), bar: (document.querySelector(".party-title-live, .party-bar") || {}).outerHTML?.slice(0, 160) })`).catch((e) => String(e)));
    if (!chip) { await screenshot("14-combat-no-chip"); return; }
    await screenshot("14-combat-live-chip");
    await page(`document.querySelector(".combat-live-chip").click(), true`);
    const pane = await until("the live combat section", () => page(`!!document.querySelector('[data-testid="combat-pane"]') && [...document.querySelectorAll('[role="tab"]')].some((t) => t.innerText.trim() === "Play" && t.getAttribute("aria-selected") === "true")`), 30000).catch(() => false);
    check("the Live chip opens the live combat section on the Play tab", !!pane);
    if (!pane) return;
    const shown = await page(`(() => { const t = document.querySelector('[data-testid="combat-pane"]').textContent; return ["Round 2", "Goblin", "Take 2 slashing damage?"].filter((w) => !t.includes(w)); })()`);
    check("the sub-tab shows the round, the turn order and the damage prompt", shown.length === 0, shown.length ? `missing ${shown.join(", ")}` : "");
    const sideways = await page(`document.documentElement.scrollWidth - innerWidth`);
    check("the Combat sub-tab fits the phone's width", sideways <= 0, `${sideways} px too wide`);
    await screenshot("15-combat-pane");
    const before = (await api(token, "get_snapshot", { characterId: hero.id })).play.hitPoints.current;
    await smoke("clickIn('Take 2 slashing', 'Apply')");
    const after = await until("the damage on the server", async () => {
      const hp = (await api(token, "get_snapshot", { characterId: hero.id })).play.hitPoints.current;
      return hp !== before ? hp : null;
    }, 15000, 500).catch(() => null);
    check("Apply takes the damage on the player's own character (D7 a)", after === Math.max(0, before - 2), `${before} -> ${after}`);
    await screenshot("16-combat-applied");

    // No generic Summon button and no monster stat block on the player's screen (user, 2026-11-14).
    const extras = await page(`[...document.querySelectorAll('[data-testid="combat-pane"] button')].filter((b) => /^Summon|stat block/.test(b.textContent + " " + (b.getAttribute("aria-label") || ""))).length`).catch(() => -1);
    check("the player's screen has no Summon button and no monster stat block", extras === 0, `${extras}`);

    // Background and back: what changed meanwhile (no event reached the app) shows when it returns.
    await page(`(() => { const s = window.__smokeCombat; s.pseq += 5; s.turn = { kind: "combatant", id: "me", round: 4 }; return true; })()`);
    adbShell("input keyevent KEYCODE_HOME");
    await sleep(2500);
    adbShell(`am start -n ${ACTIVITY}`);
    await sleep(1500);
    await connectPage().catch(() => null);
    const woke = await until("the view after coming back", () => page(`(() => { const p = document.querySelector('[data-testid="combat-pane"]'); return !!p && p.textContent.includes("Round 4") && p.textContent.includes("Your turn"); })()`), 15000, 500).catch(() => false);
    check("back from the background, the live combat section shows what changed meanwhile", !!woke, woke ? "" : (await page(`(document.querySelector('[data-testid="combat-pane"]') || document.body).innerText.slice(0, 120)`).catch(() => "")));
    await screenshot("17-combat-after-background");
  } finally {
    if (shimmed) await page(`(() => { if (window.__smokeRealFetch) window.fetch = window.__smokeRealFetch; delete window.__smokeCombat; return true; })()`).catch(() => null);
    await rest(token, "DELETE", `/api/parties/${party.id}`).catch(() => null);
    await page(`location.hash = "#/characters"`).catch(() => null);
    await page(`window.dispatchEvent(new Event("online")), true`).catch(() => null);
  }
}

/** Runs in the sheet preview frame: what the page-footer and ruled-line checks need. */
const SHEET_PROBE = `(() => {
  const problems = [];
  const pages = [...document.querySelectorAll(".page")];
  const name = (el) => (el.className || el.tagName) + ' "' + (el.textContent || "").trim().slice(0, 24) + '"';
  pages.forEach((p, i) => {
    const foot = p.querySelector(".foot");
    if (!foot) return;
    const limit = foot.getBoundingClientRect().top;
    for (const el of p.querySelectorAll(".content *")) {
      if (el.closest("svg")) continue;
      const b = el.getBoundingClientRect();
      if (b.height && b.bottom > limit + 0.5) { problems.push("page " + (i + 1) + ": " + name(el) + " runs into the footer"); break; }
    }
    for (const sec of p.querySelectorAll(".sec:not(.more-spells)")) {
      if (sec.scrollHeight > sec.clientHeight + 1) problems.push("page " + (i + 1) + ": " + name(sec) + " is shorter than its text");
    }
  });
  const lined = document.querySelector(".lined");
  return { pages: pages.length, problems, linePx: lined ? parseFloat(getComputedStyle(lined).lineHeight) : null,
    fontPx: parseFloat(getComputedStyle(document.body).fontSize) };
})()`;

/** Evaluates `expression` inside the sheet preview iframe (sandboxed, so only DevTools can look in). */
async function inPreview(expression) {
  const tree = await cdp("Page.getFrameTree");
  const frames = [];
  const walk = (n) => { frames.push(n.frame); (n.childFrames ?? []).forEach(walk); };
  walk(tree.frameTree);
  const frame = frames.find((f) => f.parentId && /about:srcdoc/.test(f.url));
  if (!frame) return null;
  const world = await cdp("Page.createIsolatedWorld", { frameId: frame.id, worldName: "smoke-sheet" });
  const r = await cdp("Runtime.evaluate", { expression, contextId: world.executionContextId, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
}

/**
 * The sheet preview at Android font sizes 1.0 and 1.15. The app's web view keeps its text at 100 %
 * (party/android/java/app/borealis/party/FixedTextZoom.kt): before, the font size scaled the sheet's text and line
 * heights but not its ruled lines and boxes, so rows left their lines and page 1's lists ran past the page footer. The
 * ruled lines are 11 pt apart (14.67 CSS px before the preview's scale), so the text's line height must be exactly that.
 */
async function sheetPreview(token) {
  const hero = (await api(token, "list_characters")).find((c) => c.name === "Test Hero");
  if (!check("the imported character is on the server for the sheet", !!hero)) return;
  // Long story text, with Organisation, Allies and Notes after it (fix/sheet-overflow: it must not run over them).
  const story = "I kept a careful list of every promise I made and every one I broke, and I read it aloud whenever the night was quiet enough to hear myself think.";
  await api(token, "load_character", { characterId: hero.id });
  await api(token, "update_details", { characterId: hero.id, patch: {
    backstory: Array(8).fill(story).join("\n\n"), organization: "The wardens of the northern lighthouse.",
    allies: `${story} ${story}`, notesLeft: `${story} ${story}`,
  } });
  await api(token, "save_character", { characterId: hero.id });
  await api(token, "close_character", { characterId: hero.id, discard: false });
  const original = adbShell("settings get system font_scale");
  fontScaleToRestore = original && original !== "null" ? original : "1.0";
  try {
    for (const scale of ["1.0", "1.15"]) {
      adbShell(`settings put system font_scale ${scale}`);
      adbShell(`am force-stop ${PACKAGE}`);
      adbShell(`am start -W -n ${ACTIVITY}`);
      await until("the app after the font size change", async () => {
        try { await connectPage(); return await page("!!localStorage.getItem('party.session') && document.readyState === 'complete'"); } catch { return false; }
      }, 60000, 2000);
      await sleep(2500);
      for (const label of ["Close", "Dismiss", "OK", "Got it"]) await smoke(`click(${JSON.stringify(label)}, true)`).catch(() => false);
      const zoom = await page(`(() => { const d = document.createElement("div"); d.style.cssText = "font-size:10px;position:absolute"; d.textContent = "x"; document.body.append(d); const px = parseFloat(getComputedStyle(d).fontSize); d.remove(); return px; })()`);
      await page(`location.hash = "#/characters"`);
      const opened = await until(`${hero.name} opened`, async () => {
        const state = await page(`(() => {
          const open = [...document.querySelectorAll("button")].find((b) => b.getAttribute("aria-label") === ${JSON.stringify(`Open ${hero.name}`)});
          if (open) { open.click(); return "open"; }
          const tile = document.querySelector('[data-testid="character-tile-${hero.id}"]');
          if (tile) tile.click();
          return tile ? "tile" : "none";
        })()`);
        return state === "open";
      }, 30000, 1000).catch(() => false);
      await sleep(2500);
      await page(`location.hash = "#/sheet?preview=1"`);
      const found = await until("the sheet preview", async () => {
        const r = await inPreview(SHEET_PROBE).catch(() => null);
        return r && r.pages > 0 ? r : null;
      }, 60000, 1500).catch(() => null);
      await sleep(1000);
      await screenshot(`04b-sheet-preview-font-${scale}`);
      await inPreview(`(() => { const p = document.querySelectorAll(".page")[1]; if (p) p.scrollIntoView(); return true; })()`).catch(() => null);
      await sleep(800);
      await screenshot(`04c-sheet-preview-page-2-font-${scale}`);
      log(`      font_scale ${scale}: page text 10px -> ${zoom}px; preview ${JSON.stringify(found)}`);
      check(`font size ${scale}: the app's text is not scaled by Android's font size`, zoom === 10, `${zoom}px`);
      check(`font size ${scale}: the sheet preview opens`, !!opened && !!found, found ? `${found.pages} pages` : "no preview");
      if (found) {
        check(`font size ${scale}: the sheet's text sits on its 11 pt ruled lines`, Math.abs(found.linePx - 44 / 3) < 0.05, `line height ${found.linePx}px`);
        check(`font size ${scale}: no sheet text runs over a section or the page footer`, found.problems.length === 0, found.problems.join(" | "));
      }
      await smoke("click('Close', true)").catch(() => false);
    }
  } finally {
    adbShell(`settings put system font_scale ${fontScaleToRestore}`);
    fontScaleToRestore = null;
  }
}

/**
 * The phone layout in the real app (plan D10): the account row (name, Sync, Account, Admin, Sign out) is not on screen,
 * those rows are under More, and the START sub-tabs have exactly one current item, Sources included (/content/sources
 * is inside /content). The browser rig can't see this: it has no Tauri.
 */
async function phoneLayout() {
  const env = await page(`({ html: document.documentElement.className, tauri: "__TAURI_INTERNALS__" in window,
    screen: [screen.width, screen.height], inner: [innerWidth, innerHeight], ua: navigator.userAgent })`);
  log(`      page: ${JSON.stringify(env)}`);
  check("the app has the phone layout (html.android.phone)", /\bandroid\b/.test(env.html) && /\bphone\b/.test(env.html), env.html);
  const bar = await page(`(() => {
    const bar = document.querySelector(".party-bar");
    const shown = (el) => !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";
    return { shown: shown(bar), buttons: bar ? [...bar.querySelectorAll("button, a")].filter(shown).map((b) => b.textContent.trim()) : [] };
  })()`);
  check("the account row is not on screen", !bar.shown, bar.shown ? `shows ${bar.buttons.join(" | ")}` : "");
  await page(`location.hash = "#/more"`);
  const rows = await until("More's account rows", async () => {
    const r = await page(`[...document.querySelectorAll('[aria-label="Account"] .v5-row')].map((e) => e.textContent.trim())`);
    return r.length ? r : null;
  }, 15000).catch(() => []);
  check("More has Account, Admin and Sign out", ["Account", "Admin", "Sign out"].every((n) => rows.some((r) => r.startsWith(n))), rows.join(" | "));
  for (const [route, label] of [["/content/sources", "Sources"], ["/content", "Additional Content"], ["/content/sources", "Sources"], ["/characters", "Character Collection"]]) {
    await page(`location.hash = ${JSON.stringify(`#${route}`)}`);
    const current = await until(`the ${label} sub-tab`, async () => {
      const c = await page(`[...document.querySelectorAll('nav[aria-label="Start sections"] [aria-current="page"]')].map((e) => e.textContent.trim())`);
      return c.includes(label) ? c : null;
    }, 15000).catch(() => []);
    check(`on ${route} only ${label} is the current sub-tab`, current.length === 1 && current[0] === label, current.join(" + ") || "none");
  }
  await page(`location.hash = "#/content/sources"`);
  await sleep(800);
  await screenshot("02b-sources");

  // The AVD's own screen (BalanceLab_Tablet: 1280x800 CSS) gets upstream's desktop shell, without the phone layer: the
  // account row stays off there too in the Android app, and More still has the rows. `--no-wide` skips it: on some
  // emulator images the resize stalls the web view (a screenshot gets no answer), 0.2.0 included.
  if (args["no-wide"]) { log("      (--no-wide: no wide-window check)"); return; }
  adbShell("wm size reset");
  adbShell("wm density reset");
  await resized("the wide window", (w) => w >= 900).catch(() => false);
  await page(`location.hash = "#/characters"`);
  await sleep(1500);
  const wide = await page(`({ html: document.documentElement.className, inner: [innerWidth, innerHeight],
    bar: [...document.querySelectorAll(".party-bar")].filter((b) => b.getClientRects().length).map((b) => b.textContent.trim()) })`);
  log(`      wide: ${JSON.stringify(wide)}`);
  await screenshot("02c-wide");
  if (wide.inner[0] >= 900) {
    check("in a wide window too, the account row is not on screen", wide.bar.length === 0, wide.bar.join(" / "));
    await page(`location.hash = "#/more"`);
    const wideRows = await until("More's account rows (wide)", async () => {
      const r = await page(`[...document.querySelectorAll('[aria-label="Account"] .v5-row')].map((e) => e.textContent.trim())`);
      return r.length ? r : null;
    }, 15000).catch(() => []);
    check("in a wide window, More has Account, Admin and Sign out", ["Account", "Admin", "Sign out"].every((n) => wideRows.some((r) => r.startsWith(n))), wideRows.join(" | "));
  } else {
    log("      (this AVD's own screen is phone-sized: no wide-window check)");
  }
  adbShell("wm size 1080x2340");
  adbShell("wm density 450");
  await resized("the phone-sized window", (w) => w < 900);
  await page(`location.hash = "#/characters"`);
  await sleep(1500);
}

let failed = false;
let limit;
/** The device's own font size while sheetPreview has changed it (restored even when the run is stopped). */
let fontScaleToRestore = null;
try {
  await Promise.race([
    main(),
    new Promise((_, reject) => { limit = setTimeout(() => reject(new Error(`stopped after the ${maxMinutes} min limit (--max-minutes)`)), maxMinutes * 60000); }),
  ]);
} catch (e) {
  failed = true;
  check("the run finished", false, String(e?.message ?? e));
  try { await screenshot("99-failure"); } catch { /* no page */ }
} finally {
  clearTimeout(limit);
  ws?.close();
  // sheetPreview puts the device's font size back itself; a run stopped by --max-minutes never reaches its finally.
  if (fontScaleToRestore) run(adbPath, ["shell", `settings put system font_scale ${fontScaleToRestore}`], { allowFail: true });
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
