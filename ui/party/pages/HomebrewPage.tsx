import { type FormEvent, useCallback, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip, EmptyState } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Modal } from "../../src/components/Modal";
import { errorMessage } from "../../src/state/useAsync";
import {
  buildIndex, createPack, deletePack, exportZip, listPacks, listTypes, loadPack, updatePack,
  type GeneratedFile, type PackDetail, type PackSummary, type TypeSpec,
} from "../api/homebrew";
import "./party.css";
import "./homebrew.css";

export const HOMEBREW_PATH = "/homebrew";
export const packPath = (pack: string) => `${HOMEBREW_PATH}/${encodeURIComponent(pack)}`;
export const elementPath = (pack: string, id: string) => `${packPath(pack)}/e/${encodeURIComponent(id)}`;
export const newElementPath = (pack: string, type: string) => `${packPath(pack)}/new/${encodeURIComponent(type)}`;

/** A new pack: its name (the address and file names come from it and never change). */
function NewPackDialog({ onClose, onMade }: { onClose: () => void; onMade: (p: PackDetail) => void }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    try {
      onMade(await createPack({ name: name.trim(), description: description.trim() }));
    } catch (err) {
      setError(errorMessage(err));
      setBusy(false);
    }
  };
  return (
    <Modal title="New homebrew pack" onClose={onClose}
      footer={<>
        <Button onClick={onClose}>Cancel</Button>
        <Button variant="primary" disabled={busy || !name.trim()} onClick={() => void submit()}>Create</Button>
      </>}>
      <form className="party-form" onSubmit={(e) => void submit(e)}>
        <label className="party-field">
          Name
          <input className="input" aria-label="Pack name" maxLength={80} value={name} onChange={(e) => setName(e.target.value)} required />
        </label>
        <label className="party-field">
          Description
          <textarea className="input" aria-label="Pack description" rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
        </label>
        <p className="muted party-note">The pack's file names come from its first name and stay the same if you rename it.</p>
        {error && <Alert tone="danger">{error}</Alert>}
      </form>
    </Modal>
  );
}

/** The account's homebrew packs (0.3.1): open one, or make the first. */
export function HomebrewPage() {
  const navigate = useNavigate();
  const [packs, setPacks] = useState<PackSummary[] | undefined>(undefined);
  const [error, setError] = useState("");
  const [creating, setCreating] = useState(false);
  useEffect(() => {
    listPacks().then(setPacks, (e) => setError(errorMessage(e)));
  }, []);
  return (
    <div className="party hb-page">
      <div className="party-line hb-head">
        <h2 className="grow hb-title">Homebrew</h2>
        <Button small icon="plus" variant="primary" onClick={() => setCreating(true)}>New pack</Button>
      </div>
      <p className="muted party-note">
        Make your own spells, items, races, subclasses and monsters. They are written as standard Aurora content and
        checked by the same engine that loads your characters. Build a pack into an index for your own repository.
      </p>
      {error && <Alert tone="danger">{error}</Alert>}
      {packs === undefined && !error && <p className="muted">Loading…</p>}
      {packs?.length === 0 && (
        <EmptyState title="No homebrew yet">
          <Button variant="primary" onClick={() => setCreating(true)}>Create your homebrew pack</Button>
        </EmptyState>
      )}
      {packs && packs.length > 0 && (
        <ul className="party-list">
          {packs.map((p) => (
            <li key={p.id}>
              <Link className="hb-row" to={packPath(p.id)} aria-label={`Open ${p.name}`}>
                <span className="grow">
                  <strong>{p.name}</strong>
                  <span className="muted party-note">
                    {p.elementCount === 1 ? "1 element" : `${p.elementCount} elements`} · version {p.version}
                  </span>
                </span>
              </Link>
            </li>
          ))}
        </ul>
      )}
      {creating && <NewPackDialog onClose={() => setCreating(false)} onMade={(p) => { setCreating(false); void navigate(packPath(p.id)); }} />}
    </div>
  );
}

function save(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * An index for the author's own repository (a GitHub repository): its address, a preview of the index, the zip. This
 * server never hosts or publishes an index (D47); an agent on the author's machine can also build it (index_build) and
 * push it.
 */
function ExportCard({ pack }: { pack: PackDetail }) {
  const [base, setBase] = useState("");
  const [files, setFiles] = useState<GeneratedFile[] | null>(null);
  const [error, setError] = useState("");
  const preview = async () => {
    setError("");
    try { setFiles((await buildIndex(pack.id, base.trim())).files); } catch (e) { setError(errorMessage(e)); }
  };
  const download = async () => {
    setError("");
    try { save(await exportZip(pack.id, base.trim()), `${pack.id.replace(/^hb-/, "")}.zip`); } catch (e) { setError(errorMessage(e)); }
  };
  return (
    <Card title="Index for your own repository">
      <div className="party-form">
        <label className="party-field">
          Where the files will be (https folder)
          <input className="input hb-mono" aria-label="Hosting address" placeholder="https://raw.githubusercontent.com/you/repo/main/"
            inputMode="url" value={base} onChange={(e) => setBase(e.target.value)} />
        </label>
        <p className="muted party-note">
          The zip holds the .index and a folder of XML files, each pointing at its own address there. Put them in your
          repository; others then add the .index address under Additional Content › Indexes. This server doesn't host them.
        </p>
        <div className="party-line">
          <Button onClick={() => void preview()} disabled={pack.elements.length === 0 || !base.trim()}>Show the index</Button>
          <Button variant="primary" onClick={() => void download()} disabled={pack.elements.length === 0 || !base.trim()}>Download zip</Button>
        </div>
        {error && <Alert tone="danger">{error}</Alert>}
        {files && <pre className="hb-xml-view" aria-label="Index preview">{files[0]?.text}</pre>}
      </div>
    </Card>
  );
}

/** Name, description, author and version of a pack. */
function DetailsDialog({ pack, onClose, onSaved }: { pack: PackDetail; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(pack.name);
  const [description, setDescription] = useState(pack.description);
  const [author, setAuthor] = useState(pack.author);
  const [authorUrl, setAuthorUrl] = useState(pack.authorUrl);
  const [version, setVersion] = useState(pack.version);
  const [error, setError] = useState("");
  const submit = async () => {
    try {
      await updatePack(pack.id, { name, description, author, authorUrl, ...(version !== pack.version ? { version } : {}) });
      onSaved();
    } catch (e) { setError(errorMessage(e)); }
  };
  return (
    <Modal title="Pack details" onClose={onClose}
      footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" onClick={() => void submit()}>Save</Button></>}>
      <div className="party-form">
        <label className="party-field">Name<input className="input" aria-label="Pack name" value={name} onChange={(e) => setName(e.target.value)} /></label>
        <label className="party-field">Description<textarea className="input" aria-label="Pack description" rows={3} value={description} onChange={(e) => setDescription(e.target.value)} /></label>
        <label className="party-field">Author<input className="input" aria-label="Author" value={author} onChange={(e) => setAuthor(e.target.value)} /></label>
        <label className="party-field">Author's page (https)<input className="input" aria-label="Author's page" inputMode="url" value={authorUrl} onChange={(e) => setAuthorUrl(e.target.value)} /></label>
        <label className="party-field">Version<input className="input" aria-label="Version" value={version} onChange={(e) => setVersion(e.target.value)} /></label>
        <p className="muted party-note">Every saved change raises the last number by itself. Set it yourself for a bigger change, like 2.0.0.</p>
        {error && <Alert tone="danger">{error}</Alert>}
      </div>
    </Modal>
  );
}

/** One pack: its elements by type, a menu of types to add, and the index for the author's own repository. */
export function HomebrewPackPage() {
  const { pack: packId = "" } = useParams();
  const navigate = useNavigate();
  const confirm = useConfirm();
  const [pack, setPack] = useState<PackDetail | null | undefined>(undefined);
  const [types, setTypes] = useState<TypeSpec[]>([]);
  const [error, setError] = useState("");
  const [adding, setAdding] = useState("");
  const [editing, setEditing] = useState(false);
  const load = useCallback(async () => {
    try { setPack(await loadPack(packId)); setError(""); } catch (e) {
      if ((e as { code?: string }).code === "not_found") setPack(null); else setError(errorMessage(e));
    }
  }, [packId]);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => { listTypes().then(setTypes, () => setTypes([])); }, []);

  if (pack === null) {
    return <div className="party hb-page"><Alert tone="info">There is no such homebrew pack.</Alert><Link className="btn" to={HOMEBREW_PATH}>Homebrew</Link></div>;
  }
  const remove = async () => {
    if (!pack) return;
    const ok = await confirm({
      title: `Delete ${pack.name}?`,
      body: "Its elements leave your content, and characters that use them show them as missing.",
      confirmLabel: "Delete pack",
      danger: true,
    });
    if (ok !== "confirm") return;
    try { await deletePack(pack.id); void navigate(HOMEBREW_PATH); } catch (e) { setError(errorMessage(e)); }
  };
  const label = (type: string) => types.find((t) => t.type === type)?.label ?? type;
  const byType = new Map<string, PackDetail["elements"]>();
  for (const e of pack?.elements ?? []) byType.set(e.type, [...(byType.get(e.type) ?? []), e]);
  return (
    <div className="party hb-page">
      <div className="party-line hb-head">
        <Link className="btn btn-sm" to={HOMEBREW_PATH} aria-label="Back to homebrew">Homebrew</Link>
        <h2 className="grow hb-title">{pack?.name ?? "…"}</h2>
        {pack && <Button small onClick={() => setEditing(true)}>Details</Button>}
      </div>
      {error && <Alert tone="danger">{error}</Alert>}
      {pack && (
        <>
          <p className="muted party-note">Version {pack.version}{pack.description ? ` · ${pack.description}` : ""}</p>
          <Card title="Elements">
            <div className="party-line hb-add">
              <select className="input grow" aria-label="Element type to add" value={adding} onChange={(e) => setAdding(e.target.value)}>
                <option value="">Add an element…</option>
                {types.map((t) => <option key={t.type} value={t.type}>{t.label}</option>)}
              </select>
              <Button variant="primary" icon="plus" disabled={!adding} onClick={() => void navigate(newElementPath(pack.id, adding))}>Add</Button>
            </div>
            {pack.elements.length === 0 && <EmptyState title="No elements yet">Choose a type above to make the first one.</EmptyState>}
            {[...byType.entries()].map(([type, rows]) => (
              <section key={type} className="hb-group">
                <h3 className="hb-group-title">{label(type)}</h3>
                <ul className="party-list">
                  {rows.map((e) => (
                    <li key={e.id}>
                      <Link className="hb-row" to={elementPath(pack.id, e.id)} aria-label={`Edit ${e.name}`}>
                        <span className="grow"><strong>{e.name}</strong><span className="muted party-note hb-mono">{e.id}</span></span>
                        <Chip>r{e.revision}</Chip>
                      </Link>
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </Card>
          <ExportCard pack={pack} />
          <div className="party-line"><Button variant="danger" onClick={() => void remove()}>Delete pack</Button></div>
          {editing && <DetailsDialog pack={pack} onClose={() => setEditing(false)} onSaved={() => { setEditing(false); void load(); }} />}
        </>
      )}
    </div>
  );
}
