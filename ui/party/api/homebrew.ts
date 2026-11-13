import { serverFetch, serverHooks } from "./http";
import { loadSession } from "./session";

/**
 * Homebrew authoring and index building (0.3.1 part D): the server's `/api/homebrew/*` routes as
 * party/docs/homebrew-index.md describes them. The server never hosts or publishes an index (D47). Every call goes through
 * {@link homebrewDeps}, so tests answer them from a fake server.
 */

export interface AttrSpec { name: string; label: string; options?: string[] }
export interface FieldSpec {
  setter: string;
  label: string;
  kind: "text" | "number" | "bool" | "select" | "cost" | "weight";
  options?: string[];
  required: boolean;
  attr?: AttrSpec;
}
export interface TypeSpec {
  type: string;
  label: string;
  file: string;
  idWord: string;
  supports?: string;
  requirements: boolean;
  sheet: boolean;
  fields: FieldSpec[];
}

export interface SetterForm { name: string; value: string; attrs?: Record<string, string> }
export interface RuleForm { kind: string; attrs: Record<string, string> }
export interface SheetForm { display?: boolean; usage?: string; action?: string; description?: string }
export interface ElementForm {
  type: string;
  id?: string;
  name: string;
  source?: string;
  description: string;
  supports?: string;
  prerequisite?: string;
  requirements?: string;
  setters?: SetterForm[];
  sheet?: SheetForm;
  rules?: RuleForm[];
  extraXml?: string;
}

export interface Issue {
  severity: "error" | "warning" | "info";
  code: string;
  message: string;
  element?: string;
  file?: string;
  line?: number;
  elementLine?: number;
}
export interface Report { ok: boolean; errors: number; warnings: number; converted: number; issues: Issue[] }

export interface PackSummary {
  id: string;
  name: string;
  description: string;
  author: string;
  authorUrl: string;
  version: string;
  elementCount: number;
  updatedAt: number;
}
export interface ElementRow { id: string; type: string; name: string; file: string; revision: number; updatedAt: number }
export interface PackDetail extends PackSummary { elements: ElementRow[]; files: Record<string, string> }
export interface StoredElement extends ElementRow { xml: string; form: ElementForm | null; createdAt: number }
export interface Saved extends StoredElement { report: Report; pack: PackSummary }
export interface GeneratedFile { path: string; text: string }

/** What the server sends with a refused element (422 `invalid_homebrew`). */
export interface Refusal { message: string; report?: Report; xml?: string }

export const homebrewDeps = {
  call: (method: string, path: string, body?: unknown): Promise<unknown> => {
    const session = loadSession();
    if (!session) return Promise.reject(new Error("Not signed in."));
    return serverFetch(session, method, path, body);
  },
  /** The raw reply of a GET (the zip export), or the error message. */
  download: async (path: string): Promise<Blob> => {
    const session = loadSession();
    if (!session) throw new Error("Not signed in.");
    const response = await fetch(`${session.baseUrl}${path}`, { headers: { authorization: `Bearer ${session.token}` } });
    if (!response.ok) {
      const text = await response.text();
      let message = `The server answered ${response.status}.`;
      try { message = String((JSON.parse(text) as { message?: string }).message ?? message); } catch { /* not JSON */ }
      throw new Error(message);
    }
    return response.blob();
  },
};

const call = <T>(method: string, path: string, body?: unknown) => homebrewDeps.call(method, path, body) as Promise<T>;
const pack = (id: string) => `/api/homebrew/packs/${encodeURIComponent(id)}`;
const element = (packId: string, id: string) => `${pack(packId)}/elements/${encodeURIComponent(id)}`;

export const listTypes = () => call<{ types: TypeSpec[] }>("GET", "/api/homebrew/types").then((r) => r.types);
export const listPacks = () => call<{ packs: PackSummary[] }>("GET", "/api/homebrew/packs").then((r) => r.packs);
export const createPack = (details: { name: string; description?: string; author?: string }) =>
  call<PackDetail>("POST", "/api/homebrew/packs", details);
export const loadPack = (id: string) => call<PackDetail>("GET", pack(id));
export const updatePack = (id: string, details: Partial<Pick<PackDetail, "name" | "description" | "author" | "authorUrl" | "version">>) =>
  call<PackDetail>("PATCH", pack(id), details);
export const deletePack = (id: string) => call<{ deleted: string }>("DELETE", pack(id));
export const loadElement = (packId: string, id: string) => call<StoredElement>("GET", element(packId, id));

/** The element as a form or as XML (exactly one). */
export type ElementInput = { form: ElementForm } | { xml: string };
export const createElement = (packId: string, input: ElementInput) => call<Saved>("POST", `${pack(packId)}/elements`, input);
export const updateElement = (packId: string, id: string, input: ElementInput) => call<Saved>("PATCH", element(packId, id), input);
export const deleteElement = (packId: string, id: string) => call<{ deleted: string; pack: PackSummary }>("DELETE", element(packId, id));
export const validateElement = (packId: string, elementId: string | undefined, input: ElementInput) =>
  call<{ report: Report; xml?: string | null }>("POST", "/api/homebrew/validate", { packId, ...(elementId ? { elementId } : {}), ...input });
export const formToXml = (packId: string, form: ElementForm) => call<{ xml: string }>("POST", "/api/homebrew/convert", { packId, form }).then((r) => r.xml);
export const xmlToForm = (xml: string) => call<{ form?: ElementForm; report?: Report }>("POST", "/api/homebrew/convert", { xml });

/** The `.index` and files for hosting in the author's own repository at `base` (required: the server never hosts). */
export const buildIndex = (packId: string, base: string) =>
  call<{ base: string; files: GeneratedFile[] }>("GET", `${pack(packId)}/index?base=${encodeURIComponent(base)}`);
export const exportZip = (packId: string, base: string) =>
  homebrewDeps.download(`${pack(packId)}/export?base=${encodeURIComponent(base)}`);

/** The report a refused save carries (the server's `invalid_homebrew`), if the error is one. */
export function refusalOf(error: unknown): Refusal | null {
  if (!error || typeof error !== "object") return null;
  const e = error as { code?: string; message?: string; details?: unknown };
  if (e.code !== "invalid_homebrew") return null;
  const details = (e.details ?? {}) as Partial<Refusal>;
  return { message: String(e.message ?? ""), report: details.report, xml: details.xml };
}

/** serverFetch keeps only `code` and `message`: a refused homebrew save keeps its report as `details`. */
export function keepReports(failure: { path: string; text: string; error: { code: string; message: string; details?: unknown } }) {
  if (!failure.path.startsWith("/api/homebrew/")) return undefined;
  try {
    const parsed = JSON.parse(failure.text) as { code?: string; report?: Report; xml?: string };
    if (parsed.code !== "invalid_homebrew") return undefined;
    return { ...failure.error, details: { report: parsed.report, xml: parsed.xml } } as typeof failure.error;
  } catch { return undefined; }
}
if (!serverHooks.fail.includes(keepReports as never)) serverHooks.fail.push(keepReports as never);

/** The setter `name` of a form, or "". */
export const setterValue = (form: ElementForm, name: string): string => form.setters?.find((s) => s.name === name)?.value ?? "";
export const setterAttr = (form: ElementForm, name: string, attr: string): string =>
  form.setters?.find((s) => s.name === name)?.attrs?.[attr] ?? "";

/** The form with setter `name` set to `value` (removed when empty), keeping its attributes. */
export function withSetter(form: ElementForm, name: string, value: string, attrs?: Record<string, string>): ElementForm {
  const setters = [...(form.setters ?? [])];
  const at = setters.findIndex((s) => s.name === name);
  const merged = { ...(at >= 0 ? setters[at]?.attrs : {}), ...attrs };
  const cleaned = Object.fromEntries(Object.entries(merged).filter(([, v]) => v !== ""));
  const next: SetterForm = { name, value, ...(Object.keys(cleaned).length ? { attrs: cleaned } : {}) };
  if (value === "" && at >= 0) setters.splice(at, 1);
  else if (value !== "" && at >= 0) setters[at] = next;
  else if (value !== "") setters.push(next);
  return { ...form, setters };
}

/** A blank form of a type, with the type's fixed category when it has only one. */
export function blankForm(spec: TypeSpec): ElementForm {
  let form: ElementForm = { type: spec.type, name: "", description: "", setters: [] };
  for (const f of spec.fields) {
    const only = f.kind === "select" && f.options?.length === 1 ? f.options[0] : undefined;
    if (only) form = withSetter(form, f.setter, only);
  }
  if (spec.sheet) form.sheet = {};
  return form;
}

/** What the form still needs before the engine will take it (names of required fields left empty). */
export function missingFields(spec: TypeSpec | undefined, form: ElementForm): string[] {
  const missing = form.name.trim() ? [] : ["Name"];
  for (const f of spec?.fields ?? []) if (f.required && !setterValue(form, f.setter).trim()) missing.push(f.label);
  return missing;
}
