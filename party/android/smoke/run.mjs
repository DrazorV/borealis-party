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
  await phoneLayout();

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
  // account row stays off there too in the Android app, and More still has the rows.
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
