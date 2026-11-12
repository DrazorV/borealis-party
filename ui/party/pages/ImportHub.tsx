import { type FormEvent, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { type Navigator, type To, UNSAFE_NavigationContext, useNavigate } from "react-router";
import type { Api, ContentProgress, FiveToolsImportReport, FiveToolsSource, ImportReport } from "../../src/api/contract";
import { Button } from "../../src/components/Button";
import { Card } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Check } from "../../src/components/Fields";
import { Icon, type IconName } from "../../src/components/Icon";
import { useImportNotice } from "../../src/components/ImportNotice";
import { fileToBase64, filesToInputs } from "../../src/lib/files";
import { FiveToolsReport } from "../../src/pages/content/FiveToolsReport";
import { FiveToolsSources } from "../../src/pages/content/FiveToolsSources";
import { describeUrlUpdate } from "../../src/pages/content/FiveToolsUrlSection";
import { ImportPreviewHost } from "../../src/pages/content/ImportPreviewDialog";
import { sourceConsentText } from "../../src/pages/content/IndexesTab";
import { describeIndexUpdate } from "../../src/pages/content/indexUpdateText";
import { useApi } from "../../src/state/ApiContext";
import { startImportPreview, useImportPreviewState } from "../../src/state/importPreview";
import { errorMessage } from "../../src/state/useAsync";
import { openBundleFromImport } from "../api/bundles";
import { loadSession } from "../api/session";
import { inboxEmpty, onInbox, sendFiles, takeFocus, takeInbox } from "../import/door";
import { KIND_LABEL, needsNotice, planImport, sentence } from "../import/plan";
import { classifyLink, LATEST_5ETOOLS_URL, type LinkKind } from "../import/sniff";
import { receiveRoute } from "./ShareReceivePage";
import "./party.css";

/**
 * One Import screen (plan I9): every kind of file and link the account can take in, so nobody has to know which of
 * Start, Additional Content (Packs, Indexes, 5etools) or File > Open bundle reads what. Files are told apart by their
 * bytes (../import/sniff.ts) and handed to the same commands those tabs use, which keep working unchanged:
 * - characters (.dnd5e, native JSON): `import_character`;
 * - Borealis bundles: `stage_bundle`, then upstream's Open bundle window;
 * - Aurora content (.xml, .index, .zip): `preview_import_pack`, then upstream's import preview, which installs;
 * - 5etools-format data (.zip, .json) and 5etools GitHub links: `stage_5etools_*`, then the books to import.
 * Index links go to `add_index` and `update_indexes`. Each file and link gets a row in one list. Failures keep the
 * server's own words (a refused private address, a too-large file as ../api/overrides/errors.ts words it).
 *
 * Every other place that starts an import (upstream's buttons and menu items, drops on the window, "Open with") opens
 * this screen through ../import/door.ts and hands over its files or link, which are taken here one pick at a time.
 */

type Tone = "working" | "success" | "warning" | "error" | "info";

/** 5etools-format data downloaded or unpacked on the server, waiting for its books to be chosen. */
interface Staged {
  token: string;
  sources: FiveToolsSource[];
}

interface Row {
  id: number;
  title: string;
  kind: string;
  tone: Tone;
  message: string;
  details: string[];
  staged?: Staged;
  /** A link that could be either: the two ways to take it. */
  choose?: string;
  /** A 5etools import with entries imported in part or skipped: its report. */
  report?: FiveToolsImportReport;
}

const ICONS: Record<Tone, IconName> = { working: "update", success: "check", warning: "alert", error: "x", info: "info" };

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const hostOf = (url: string) => { try { return new URL(url).host; } catch { return url; } };
const checkedText = (iso: string) => `Last checked at ${new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
const sameUrl = (a: string, b: string) => a.replace(/\/+$/, "") === b.replace(/\/+$/, "");

/** What the character import said, as a row. */
function characterRow(r: ImportReport): Pick<Row, "tone" | "message" | "details"> {
  const details = [...r.warnings];
  if (r.missingContent.length > 0) details.push(`${plural(r.missingContent.length, "item")} of content it uses ${r.missingContent.length === 1 ? "is" : "are"} not installed.`);
  if (r.unmappedSlots.length > 0) details.push(`${plural(r.unmappedSlots.length, "choice")} could not be placed; the builder lists ${r.unmappedSlots.length === 1 ? "it" : "them"}.`);
  return { tone: details.length > 0 ? "warning" : "success", message: `Imported ${r.summary.name}.`, details };
}

/**
 * How upstream's import preview (one dialog, a store) closes: installed, or closed without installing. Installing
 * sets it busy while the preview is still open; an install that fails clears busy and leaves the preview open.
 */
function usePreviewOutcome(): () => Promise<"installed" | "closed"> {
  const { preview, busy, report } = useImportPreviewState();
  const waiter = useRef<{ installing: boolean; resolve: (r: "installed" | "closed") => void } | null>(null);
  useEffect(() => {
    const w = waiter.current;
    if (!w) return;
    if (preview) { w.installing = busy; return; }
    // Installed: the install report (shown when it lists issues) closes first, so the next dialog isn't under it.
    if (busy || report) return;
    waiter.current = null;
    w.resolve(w.installing ? "installed" : "closed");
  }, [preview, busy, report]);
  return useCallback(() => new Promise((resolve) => { waiter.current = { installing: false, resolve }; }), []);
}

/**
 * Upstream's import preview goes to Additional Content once it installed (or added an index). Opened from here, it
 * stays on this screen instead, so the result lands in this list and the next file's dialog can follow.
 */
function StayOnImport({ children }: { children: ReactNode }) {
  const context = useContext(UNSAFE_NavigationContext);
  const value = useMemo(() => {
    if (!context) return context;
    const away = (to: To) => (typeof to === "string" ? to : to.pathname ?? "").split(/[?#]/)[0] === "/content";
    const base = context.navigator;
    const navigator: Navigator = Object.assign(Object.create(base) as Navigator, {
      push: (to: To, state?: unknown, opts?: unknown) => { if (!away(to)) base.push(to, state, opts as never); },
      replace: (to: To, state?: unknown, opts?: unknown) => { if (!away(to)) base.replace(to, state, opts as never); },
    });
    return { ...context, navigator };
  }, [context]);
  return <UNSAFE_NavigationContext.Provider value={value}>{children}</UNSAFE_NavigationContext.Provider>;
}

/** The books of staged 5etools-format data, to choose and import (as the 5etools tab does). */
function StagedBooks({ api, staged, onDone }: { api: Api; staged: Staged; onDone: (report: FiveToolsImportReport | null, error?: string) => void }) {
  const [picked, setPicked] = useState(() => staged.sources.map((s) => s.abbr));
  const [fluff, setFluff] = useState(false);
  const [running, setRunning] = useState(false);
  const run = async () => {
    setRunning(true);
    try { onDone(await api.import5etools({ request: { folder: staged.token, sources: picked, includeFluff: fluff } })); }
    catch (e) { onDone(null, errorMessage(e)); }
    finally { setRunning(false); }
  };
  return (
    <div className="party-import-books">
      <FiveToolsSources sources={staged.sources} picked={picked} onPicked={setPicked} />
      <Check label="Include fluff (descriptions and lore text)" checked={fluff} onChange={(e) => setFluff(e.target.checked)} />
      <Button variant="primary" disabled={running || picked.length === 0} onClick={() => void run()}>
        {running ? "Importing…" : `Import ${plural(picked.length, "book")}`}
      </Button>
    </div>
  );
}

/** The 5etools card, which a door can ask to show. */
const FIVETOOLS_CARD = "party-import-5etools";

export function ImportHub() {
  const api = useApi();
  const confirm = useConfirm();
  const notice = useImportNotice();
  const previewClosed = usePreviewOutcome();
  const fileRef = useRef<HTMLInputElement>(null);
  const linkRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const nextId = useRef(1);
  const [rows, setRows] = useState<Row[]>([]);
  const [link, setLink] = useState("");
  const [shareCode, setShareCode] = useState("");
  const nav = useNavigate();
  const [busy, setBusyState] = useState(false);
  // Read by work that outlives a render (the inbox is emptied in a loop).
  const busyRef = useRef(false);
  const setBusy = (b: boolean) => { busyRef.current = b; setBusyState(b); };
  const [progress, setProgress] = useState<ContentProgress | null>(null);
  useEffect(() => api.subscribe("import5etools:progress", setProgress), [api]);
  useEffect(() => api.subscribe("content:progress", setProgress), [api]);

  const add = (row: Omit<Row, "id" | "details"> & { details?: string[] }): number => {
    const id = nextId.current++;
    setRows((list) => [...list, { details: [], ...row, id }]);
    // The newest row in view, below the buttons on a phone.
    requestAnimationFrame(() => listRef.current?.lastElementChild?.scrollIntoView?.({ block: "nearest" }));
    return id;
  };
  const patch = (ids: number | number[], next: Partial<Row>) => {
    const set = new Set(Array.isArray(ids) ? ids : [ids]);
    setRows((list) => list.map((r) => (set.has(r.id) ? { ...r, ...next } : r)));
  };
  const fail = (ids: number | number[], e: unknown) => patch(ids, { tone: "error", message: sentence(errorMessage(e)), staged: undefined, choose: undefined });

  /** Scan staged 5etools data and offer its books. */
  const offerBooks = async (id: number, token: string) => {
    const scan = await api.scan5etoolsFolder({ folder: token });
    if (scan.sources.length === 0) { patch(id, { tone: "warning", message: "No books were found in it." }); return; }
    patch(id, { tone: "info", message: `${plural(scan.sources.length, "book")} found. Choose which to import.`, staged: { token, sources: scan.sources } });
  };

  const booksDone = (id: number, report: FiveToolsImportReport | null, error?: string) => {
    if (!report) { patch(id, { tone: "error", message: sentence(error ?? "Nothing was imported."), staged: undefined }); return; }
    const notes = report.partial.length + report.skipped.length;
    patch(id, {
      tone: notes > 0 ? "warning" : "success",
      message: `Imported ${plural(report.converted, "element")} into ${plural(report.packs.length, "content pack")}.`,
      details: notes > 0 ? [`${plural(notes, "entry", "entries")} imported in part or skipped:`] : [],
      report: notes > 0 ? report : undefined,
      staged: undefined,
    });
  };

  const importFiles = async (files: File[]) => {
    if (files.length === 0 || busyRef.current) return;
    setBusy(true);
    const session = loadSession();
    try {
      const plan = await planImport(files);
      const ids = new Map<File, number>();
      for (const f of files) ids.set(f, add({ title: f.name, kind: KIND_LABEL[plan.kinds.get(f) ?? "other"], tone: "working", message: "Waiting…" }));
      const id = (f: File) => ids.get(f)!;
      for (const f of plan.others) {
        patch(id(f), { tone: "error", message: "Nothing here reads this file. Use a .dnd5e, .borealis, .xml, .index, .zip or .json file." });
      }
      for (const f of plan.characters) {
        patch(id(f), { message: "Importing…" });
        try { patch(id(f), characterRow(await api.importCharacter({ fileName: f.name, contentBase64: await fileToBase64(f) }))); }
        catch (e) { fail(id(f), e); }
      }
      const content = needsNotice(plan) && await notice.ensure();
      if (needsNotice(plan) && !content) {
        for (const f of [...plan.packs, ...plan.fiveToolsFiles, ...plan.fiveToolsZips]) patch(id(f), { tone: "error", message: "Not imported: the content import notice was not accepted." });
      }
      if (content && plan.packs.length > 0) {
        const packIds = plan.packs.map(id);
        patch(packIds, { message: plan.packs.length > 1 ? "Converting together…" : "Converting…" });
        let error = "";
        const before = new Set((await api.listPacks().catch(() => [])).map((p) => p.id));
        const preview = await startImportPreview(api, { files: await filesToInputs(plan.packs) }, (m) => { error = m; });
        if (!preview) fail(packIds, { message: error || "Nothing could be converted." });
        else {
          patch(packIds, { tone: "info", message: "Check the import preview, then install it." });
          if (await previewClosed() === "installed") {
            const added = (await api.listPacks().catch(() => [])).filter((p) => !before.has(p.id)).map((p) => p.name);
            patch(packIds, { tone: "success", message: added.length > 0 ? `Installed ${added.join(", ")}.` : "Installed." });
          } else patch(packIds, { tone: "warning", message: "The import preview was closed; nothing was installed." });
        }
      }
      for (const f of plan.bundles) {
        if (!session) { fail(id(f), { message: "Not signed in." }); continue; }
        patch(id(f), { message: "Opening…" });
        try {
          const r = await openBundleFromImport(session, f.name, await fileToBase64(f));
          patch(id(f), { tone: r.warnings.length > 0 ? "warning" : "success", message: `Imported ${r.summary.name}.`, details: r.warnings });
        } catch (e) { fail(id(f), e); }
      }
      if (content && plan.fiveToolsFiles.length > 0) {
        const brewIds = plan.fiveToolsFiles.map(id);
        patch(brewIds, { message: "Reading…" });
        try {
          const pick = await api.stage5etoolsFiles({ files: await filesToInputs(plan.fiveToolsFiles) });
          const [first, ...rest] = brewIds;
          if (rest.length > 0) patch(rest, { tone: "info", message: `Read together with ${plan.fiveToolsFiles[0]!.name}; its books are listed there.` });
          await offerBooks(first!, pick.token);
        } catch (e) { fail(brewIds, e); }
      }
      for (const f of content ? plan.fiveToolsZips : []) {
        patch(id(f), { message: "Unpacking…" });
        try {
          const [input] = await filesToInputs([f]);
          await offerBooks(id(f), (await api.stage5etoolsZip({ file: input! })).token);
        } catch (e) { fail(id(f), e); }
      }
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  /** Asks before the server downloads from a host for the first time (upstream's wording). */
  const consent = async (url: string, label: string) =>
    (await confirm({ title: "Add content source", body: sourceConsentText(hostOf(url)), confirmLabel: label })) === "confirm";

  const addIndex = async (id: number, url: string) => {
    const known = (await api.listIndexes().catch(() => [])).some((i) => sameUrl(i.url, url));
    if (!known && !(await consent(url, "Add source"))) { patch(id, { tone: "warning", message: "Not added." }); return; }
    patch(id, { kind: "Index", tone: "working", message: "Adding and downloading…", choose: undefined });
    try {
      await api.addIndex({ url });
      const r = await api.updateIndexes();
      const list = await api.listIndexes().catch(() => []);
      const text = describeIndexUpdate(r, list, checkedText(new Date().toISOString()));
      const problem = list.find((i) => sameUrl(i.url, url))?.problem;
      const tone: Tone = text.tone === "error" ? "error" : text.tone === "warning" || problem ? "warning" : "success";
      // The status already names an index's problem; the row adds it only when it doesn't.
      patch(id, { tone, message: text.status, details: problem && !text.status.includes(problem) ? [problem] : [] });
    } catch (e) { fail(id, e); }
  };

  const fetchFiveTools = async (id: number, url: string) => {
    const known = (await api.list5etoolsUrls().catch(() => [])).find((u) => sameUrl(u.url, url));
    if (!(await notice.ensure())) { patch(id, { tone: "warning", message: "Not imported: the content import notice was not accepted." }); return; }
    if (!known && !(await consent(url, "Download"))) { patch(id, { tone: "warning", message: "Not downloaded." }); return; }
    patch(id, { kind: "5etools-format data", tone: "working", message: known ? "Checking for changes…" : "Downloading…", choose: undefined });
    try {
      if (known) {
        // Imported from here before: Update re-imports the same books into the same packs.
        const r = await api.update5etoolsUrl({ url: known.url });
        patch(id, { tone: "success", message: describeUrlUpdate(r.upToDate, r.report) });
      } else await offerBooks(id, (await api.stage5etoolsUrl({ url })).token);
    } catch (e) { fail(id, e); }
  };

  const take = async (kind: LinkKind, id: number, url: string) => {
    setBusy(true);
    try {
      if (kind === "index") await addIndex(id, url);
      else if (kind === "fivetools") await fetchFiveTools(id, url);
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };

  const submitLink = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    const parsed = classifyLink(link);
    if (!parsed) { add({ title: link.trim() || "Link", kind: "Link", tone: "error", message: "That is not a web address. Paste a link that starts with https://." }); return; }
    setLink("");
    if (parsed.kind === "unknown") {
      add({ title: parsed.url, kind: "Link", tone: "info", message: "Is this an index file or a folder of 5etools-format data?", choose: parsed.url });
      return;
    }
    const id = add({ title: parsed.url, kind: parsed.kind === "index" ? "Index" : "5etools-format data", tone: "working", message: "Waiting…" });
    await take(parsed.kind, id, parsed.url);
  };

  const latest = async () => {
    if (busy) return;
    const id = add({ title: "Latest 5etools data", kind: "5etools-format data", tone: "working", message: "Waiting…" });
    await take("fivetools", id, LATEST_5ETOOLS_URL);
  };

  /** What the other doors handed over (../import/door.ts), one pick at a time. */
  const draining = useRef(false);
  const drain = async () => {
    if (draining.current || busyRef.current) return;
    draining.current = true;
    try {
      while (!inboxEmpty()) {
        const inbox = takeInbox();
        await importFiles(inbox.files);
        for (const l of inbox.links) {
          const parsed = classifyLink(l.url);
          if (!parsed) { add({ title: l.url, kind: "Link", tone: "error", message: "That is not a web address. Paste a link that starts with https://." }); continue; }
          const id = add({ title: parsed.url, kind: l.kind === "index" ? "Index" : "5etools-format data", tone: "working", message: "Waiting…" });
          await take(l.kind, id, parsed.url);
        }
      }
    } finally { draining.current = false; }
  };
  /** The part of the screen the door asked for. */
  const showFocus = () => {
    const focus = takeFocus();
    if (focus === "link") { linkRef.current?.scrollIntoView?.({ block: "center" }); linkRef.current?.focus(); }
    else if (focus === "fivetools") document.getElementById(FIVETOOLS_CARD)?.scrollIntoView?.({ block: "start" });
  };
  const arrived = useRef(() => undefined as void);
  arrived.current = () => { showFocus(); void drain(); };
  useEffect(() => {
    arrived.current();
    return onInbox(() => arrived.current());
  }, []);
  // A pick that waited for the one before it.
  useEffect(() => { if (!busy) void drain(); }, [busy]);

  return (
    // Drops anywhere on the page come in through the inbox (../entry/unifyImports.ts).
    <div className="party party-import">
      <Card title="Import">
        <p className="muted">Characters (.dnd5e), Borealis bundles (.borealis), Aurora content (.xml, .index or .zip) and 5etools-format data (.zip or .json). Choose several at once: each file goes where it belongs.</p>
        <div className="party-import-actions">
          <Button variant="primary" icon="upload" disabled={busy} onClick={() => fileRef.current?.click()}>Choose files</Button>
        </div>
        {/* No accept filter: Android's web view fails on extensions it has no type for, and the files are sniffed anyway. */}
        <input ref={fileRef} type="file" multiple hidden aria-label="Choose files to import"
          onChange={(e) => { const files = Array.from(e.target.files ?? []); e.target.value = ""; sendFiles(files); }} />
      </Card>
      <Card title="Share code">
        <form className="party-import-link" onSubmit={(e) => { e.preventDefault(); if (shareCode.trim()) nav(receiveRoute(shareCode.trim())); }}>
          <input className="input" autoCapitalize="characters" autoCorrect="off" autoComplete="off" spellCheck={false}
            aria-label="Share code to enter" placeholder="ABCD-EFGH-JKMN-PQRS" value={shareCode} onChange={(e) => setShareCode(e.target.value)} />
          <Button type="submit" icon="download" disabled={!shareCode.trim()}>Enter a share code</Button>
        </form>
        <p className="muted small">Another player's characters and content, from a code they made with Share with a code.</p>
      </Card>
      <Card title="Paste a link">
        <form className="party-import-link" onSubmit={(e) => void submitLink(e)}>
          <input ref={linkRef} className="input" type="url" inputMode="url" autoCapitalize="none" autoCorrect="off" spellCheck={false}
            aria-label="Link to import" placeholder="https://" value={link} onChange={(e) => setLink(e.target.value)} />
          <Button type="submit" variant="primary" icon="download" disabled={busy || !link.trim()}>Import</Button>
        </form>
        <p className="muted small">An Aurora or Borealis .index file, or a GitHub repository (or https:// data/ folder) of 5etools-format data. The server downloads it.</p>
      </Card>
      <Card title="5etools data" id={FIVETOOLS_CARD}>
        <p className="muted">Download the newest 5etools-format data from its public GitHub repository, then choose the books. Next time it re-imports the same books when they changed. Only use content you are entitled to use.</p>
        <div className="party-import-actions">
          <Button icon="download" disabled={busy} onClick={() => void latest()}>Get the latest 5etools data</Button>
        </div>
      </Card>
      {rows.length > 0 && (
        <Card title="Results" actions={<Button small disabled={busy} onClick={() => setRows([])}>Clear</Button>}>
          <ul className="party-import-results" aria-label="Import results" ref={listRef}>
            {rows.map((r) => (
              <li key={r.id} className={`party-import-row ${r.tone}`} data-tone={r.tone}>
                <div className="party-import-head">
                  <Icon name={ICONS[r.tone]} />
                  <strong className="party-import-name">{r.title}</strong>
                  <span className="muted small party-import-kind">{r.kind}</span>
                </div>
                <p className="party-import-msg" role={r.tone === "error" ? "alert" : "status"}>
                  {r.message}
                  {r.tone === "working" && progress?.message ? <span className="muted small"> {progress.message}</span> : null}
                </p>
                {r.details.map((d) => <p key={d} className="muted small party-import-detail">{d}</p>)}
                {r.choose && (
                  <div className="party-import-actions">
                    <Button disabled={busy} onClick={() => void take("index", r.id, r.choose!)}>Add as an index</Button>
                    <Button disabled={busy} onClick={() => void take("fivetools", r.id, r.choose!)}>Import as 5etools data</Button>
                  </div>
                )}
                {r.report && <div className="party-import-report"><FiveToolsReport report={r.report} /></div>}
                {r.staged && <StagedBooks api={api} staged={r.staged} onDone={(report, error) => booksDone(r.id, report, error)} />}
              </li>
            ))}
          </ul>
        </Card>
      )}
      <StayOnImport><ImportPreviewHost /></StayOnImport>
    </div>
  );
}
