import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { Button } from "../../src/components/Button";
import { Alert, Card, Chip } from "../../src/components/Card";
import { useConfirm } from "../../src/components/Confirm";
import { Tabs } from "../../src/components/Tabs";
import { useToast } from "../../src/components/Toast";
import { errorMessage } from "../../src/state/useAsync";
import {
  blankForm, createElement, deleteElement, formToXml, listTypes, loadElement, missingFields, refusalOf, setterAttr,
  setterValue, updateElement, validateElement, withSetter, xmlToForm,
  type ElementForm, type ElementInput, type FieldSpec, type Report, type RuleForm, type TypeSpec,
} from "../api/homebrew";
import { AssistantButton } from "../assistant/AssistantButton";
import { elementPath, packPath } from "./HomebrewPage";
import "./party.css";
import "./homebrew.css";

const CURRENCIES = ["gp", "sp", "cp", "ep", "pp"];
/** The attributes the rules editor shows for each kind (others are kept as they are). */
export const RULE_KEYS: Record<string, string[]> = {
  grant: ["type", "id", "level"],
  stat: ["name", "value", "bonus", "level"],
  select: ["type", "name", "supports", "number", "level"],
};

/** The engine's findings: errors first, each with where it is. */
export function ReportView({ report }: { report: Report }) {
  if (report.ok && report.issues.length === 0) return <Alert tone="success" role="status">The engine accepts it.</Alert>;
  const sorted = [...report.issues].sort((a, b) => Number(b.severity === "error") - Number(a.severity === "error"));
  return (
    <div role="status" className="hb-report">
      <Alert tone={!report.ok ? "danger" : report.warnings ? "warning" : "success"}>
        {!report.ok
          ? `${report.errors} error${report.errors === 1 ? "" : "s"}: nothing was saved.`
          : report.warnings
            ? `The engine accepts it, with ${report.warnings} warning${report.warnings === 1 ? "" : "s"}.`
            : "The engine accepts it."}
      </Alert>
      <ul className="party-list hb-issues">
        {sorted.map((i, n) => (
          <li key={n} className={`hb-issue hb-issue-${i.severity}`}>
            <Chip tone={i.severity === "error" ? "danger" : i.severity === "warning" ? "warning" : "info"}>{i.severity}</Chip>{" "}
            <span>{i.message}</span>
            <span className="muted party-note">
              {[i.elementLine ? `line ${i.elementLine} of the element` : "", i.element ?? "", i.code].filter(Boolean).join(" · ")}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Field({ spec, form, onChange }: { spec: FieldSpec; form: ElementForm; onChange: (f: ElementForm) => void }) {
  const value = setterValue(form, spec.setter);
  const label = `${spec.label}${spec.required ? " *" : ""}`;
  const set = (v: string, attrs?: Record<string, string>) => onChange(withSetter(form, spec.setter, v, attrs));
  const attr = spec.attr;
  const attrInput = attr && (
    <label className="party-field">
      {attr.label}
      {attr.options?.length
        ? (
          <select className="input" aria-label={attr.label} value={setterAttr(form, spec.setter, attr.name)}
            onChange={(e) => set(value, { [attr.name]: e.target.value })}>
            <option value="" />
            {attr.options.map((o) => <option key={o} value={o}>{o}</option>)}
          </select>
        )
        : <input className="input" aria-label={attr.label} value={setterAttr(form, spec.setter, attr.name)} onChange={(e) => set(value, { [attr.name]: e.target.value })} />}
    </label>
  );
  switch (spec.kind) {
    case "bool":
      return (
        <>
          <label className="party-check hb-check">
            <input type="checkbox" aria-label={spec.label} checked={value === "true"} onChange={(e) => set(e.target.checked ? "true" : "")} /> {label}
          </label>
          {value === "true" && attrInput}
        </>
      );
    case "select": {
      const options = spec.options ?? [];
      return (
        <>
          <label className="party-field">
            {label}
            <select className="input" aria-label={spec.label} value={value} onChange={(e) => set(e.target.value)}>
              <option value="" />
              {!options.includes(value) && value && <option value={value}>{value}</option>}
              {options.map((o) => <option key={o} value={o}>{o}</option>)}
            </select>
          </label>
          {attrInput}
        </>
      );
    }
    case "cost":
      return (
        <div className="party-line hb-pair">
          <label className="party-field grow">
            {label}
            <input className="input" aria-label={spec.label} inputMode="decimal" value={value}
              onChange={(e) => set(e.target.value, { currency: setterAttr(form, spec.setter, "currency") || "gp" })} />
          </label>
          <label className="party-field">
            Currency
            <select className="input" aria-label="Currency" value={setterAttr(form, spec.setter, "currency") || "gp"}
              onChange={(e) => set(value, { currency: e.target.value })}>
              {CURRENCIES.map((c) => <option key={c} value={c}>{c}</option>)}
            </select>
          </label>
        </div>
      );
    default:
      return (
        <>
          <label className="party-field">
            {label}
            <input className="input" aria-label={spec.label} inputMode={spec.kind === "number" || spec.kind === "weight" ? "decimal" : undefined}
              value={value} onChange={(e) => set(e.target.value)} />
          </label>
          {attrInput}
        </>
      );
  }
}

function RuleRow({ rule, onChange, onRemove }: { rule: RuleForm; onChange: (r: RuleForm) => void; onRemove: () => void }) {
  const keys = RULE_KEYS[rule.kind] ?? Object.keys(rule.attrs);
  return (
    <li className="hb-rule">
      <div className="party-line">
        <select className="input" aria-label="Rule kind" value={rule.kind} onChange={(e) => onChange({ ...rule, kind: e.target.value })}>
          <option value="grant">grant</option>
          <option value="stat">stat</option>
          <option value="select">select</option>
        </select>
        <Button small variant="ghost" onClick={onRemove} aria-label="Remove rule">Remove</Button>
      </div>
      <div className="hb-rule-attrs">
        {keys.map((k) => (
          <label key={k} className="party-field">
            {k}
            <input className="input" aria-label={`Rule ${k}`} value={rule.attrs[k] ?? ""}
              onChange={(e) => onChange({ ...rule, attrs: { ...rule.attrs, [k]: e.target.value } })} />
          </label>
        ))}
      </div>
    </li>
  );
}

/** The form for one element type. */
export function ElementFormView({ spec, form, onChange }: { spec: TypeSpec | undefined; form: ElementForm; onChange: (f: ElementForm) => void }) {
  const known = new Set(spec?.fields.map((f) => f.setter));
  const others = (form.setters ?? []).filter((s) => !known.has(s.name));
  const sheet = form.sheet ?? {};
  const rules = form.rules ?? [];
  const setRules = (next: RuleForm[]) => onChange({ ...form, rules: next });
  return (
    <div className="party-form hb-form">
      <label className="party-field">
        Name *
        <input className="input" aria-label="Name" maxLength={200} value={form.name} onChange={(e) => onChange({ ...form, name: e.target.value })} />
      </label>
      {form.id && <p className="muted party-note hb-mono">Id {form.id} (it never changes)</p>}
      <label className="party-field">
        Description
        <textarea className="input" aria-label="Description" rows={6} value={form.description}
          onChange={(e) => onChange({ ...form, description: e.target.value })} />
        <span className="muted party-note">A blank line starts a new paragraph. Start with &lt; to write XHTML.</span>
      </label>
      {spec?.supports && (
        <label className="party-field">
          {spec.supports}
          <input className="input" aria-label="Supports" value={form.supports ?? ""} onChange={(e) => onChange({ ...form, supports: e.target.value })} />
        </label>
      )}
      {spec?.requirements && (
        <>
          <label className="party-field">
            Prerequisite (shown)
            <input className="input" aria-label="Prerequisite" value={form.prerequisite ?? ""} onChange={(e) => onChange({ ...form, prerequisite: e.target.value })} />
          </label>
          <label className="party-field">
            Requirements (ids, checked)
            <input className="input hb-mono" aria-label="Requirements" value={form.requirements ?? ""} onChange={(e) => onChange({ ...form, requirements: e.target.value })} />
          </label>
        </>
      )}
      {spec?.fields.map((f) => <Field key={f.setter} spec={f} form={form} onChange={onChange} />)}
      {others.length > 0 && (
        <fieldset className="hb-fieldset">
          <legend>Other setters</legend>
          {others.map((s) => (
            <label key={s.name} className="party-field">
              {s.name}
              <input className="input" aria-label={`Setter ${s.name}`} value={s.value} onChange={(e) => onChange(withSetter(form, s.name, e.target.value))} />
            </label>
          ))}
        </fieldset>
      )}
      {spec?.sheet && (
        <fieldset className="hb-fieldset">
          <legend>On the sheet</legend>
          <label className="party-field">
            Usage (e.g. 1/Long Rest)
            <input className="input" aria-label="Usage" value={sheet.usage ?? ""} onChange={(e) => onChange({ ...form, sheet: { ...sheet, usage: e.target.value } })} />
          </label>
          <label className="party-field">
            Action (e.g. Bonus Action)
            <input className="input" aria-label="Action" value={sheet.action ?? ""} onChange={(e) => onChange({ ...form, sheet: { ...sheet, action: e.target.value } })} />
          </label>
          <label className="party-field">
            Sheet text
            <textarea className="input" aria-label="Sheet text" rows={2} value={sheet.description ?? ""}
              onChange={(e) => onChange({ ...form, sheet: { ...sheet, description: e.target.value } })} />
          </label>
          <label className="party-check hb-check">
            <input type="checkbox" aria-label="Hide on the sheet" checked={sheet.display === false}
              onChange={(e) => onChange({ ...form, sheet: { ...sheet, display: e.target.checked ? false : undefined } })} /> Hide on the sheet
          </label>
        </fieldset>
      )}
      <fieldset className="hb-fieldset">
        <legend>Rules</legend>
        <ul className="party-list">
          {rules.map((r, i) => (
            <RuleRow key={i} rule={r}
              onChange={(next) => setRules(rules.map((x, j) => (j === i ? next : x)))}
              onRemove={() => setRules(rules.filter((_, j) => j !== i))} />
          ))}
        </ul>
        <Button small icon="plus" onClick={() => setRules([...rules, { kind: "grant", attrs: {} }])}>Add a rule</Button>
      </fieldset>
      {form.extraXml && <p className="muted party-note">This element has more XML the form has no fields for; it is kept. Edit it in the XML tab.</p>}
    </div>
  );
}

/** One element: a form per type and an XML tab, checked by the engine before it is saved. */
export function HomebrewEditor() {
  const { pack: packId = "", element: elementId, type: newType } = useParams();
  const navigate = useNavigate();
  const confirm = useConfirm();
  const toast = useToast();
  const [types, setTypes] = useState<TypeSpec[]>([]);
  const [form, setForm] = useState<ElementForm | null>(null);
  const [xml, setXml] = useState("");
  const [tab, setTab] = useState<"form" | "xml">("form");
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const all = await listTypes();
        if (!live) return;
        setTypes(all);
        if (elementId) {
          const e = await loadElement(packId, elementId);
          if (!live) return;
          setXml(e.xml);
          if (e.form) setForm(e.form); else setTab("xml");
        } else {
          const spec = all.find((t) => t.type === newType);
          if (!spec) { setError("The editor doesn't make that type."); return; }
          setForm(blankForm(spec));
        }
        setLoaded(true);
      } catch (e) { if (live) setError(errorMessage(e)); }
    })();
    return () => { live = false; };
  }, [packId, elementId, newType]);

  const spec = types.find((t) => t.type === (form?.type ?? newType));
  const input = (): ElementInput => (tab === "xml" ? { xml } : { form: form ?? { type: newType ?? "", name: "", description: "" } });

  const switchTab = async (next: string) => {
    if (next === tab) return;
    setError("");
    try {
      if (next === "xml" && form) {
        setXml(await formToXml(packId, form));
        setTab("xml");
      } else if (next === "form") {
        const back = await xmlToForm(xml);
        if (back.form) { setForm(back.form); setTab("form"); setReport(null); } else if (back.report) setReport(back.report);
      }
    } catch (e) { setError(errorMessage(e)); }
  };

  const check = async () => {
    setBusy(true);
    setError("");
    try { setReport((await validateElement(packId, elementId, input())).report); } catch (e) { setError(errorMessage(e)); }
    setBusy(false);
  };

  const save = async () => {
    if (tab === "form" && form) {
      const missing = missingFields(spec, form);
      if (missing.length) { setError(`Fill in: ${missing.join(", ")}.`); return; }
    }
    setBusy(true);
    setError("");
    try {
      const saved = elementId ? await updateElement(packId, elementId, input()) : await createElement(packId, input());
      setReport(saved.report);
      setXml(saved.xml);
      if (saved.form) setForm(saved.form);
      toast.push(`Saved: version ${saved.pack.version}.`);
      if (!elementId) void navigate(elementPath(packId, saved.id), { replace: true });
    } catch (e) {
      const refusal = refusalOf(e);
      if (refusal?.report) setReport(refusal.report); else setError(errorMessage(e));
    }
    setBusy(false);
  };

  const remove = async () => {
    if (!elementId) return;
    const ok = await confirm({
      title: "Delete this element?",
      body: "Characters that picked it will show it as missing.",
      confirmLabel: "Delete",
      danger: true,
    });
    if (ok !== "confirm") return;
    try { await deleteElement(packId, elementId); void navigate(packPath(packId)); } catch (e) {
      const refusal = refusalOf(e);
      if (refusal?.report) setReport(refusal.report); else setError(errorMessage(e));
    }
  };

  return (
    <div className="party hb-page hb-editor">
      <div className="party-line hb-head">
        <Link className="btn btn-sm" to={packPath(packId)} aria-label="Back to the pack">Pack</Link>
        <h2 className="grow hb-title">{form?.name || (elementId ? "Element" : `New ${spec?.label.toLowerCase() ?? "element"}`)}</h2>
      </div>
      {error && <Alert tone="danger">{error}</Alert>}
      {loaded && (
        <Card>
          <Tabs label="Edit as" idPrefix="hb-tab" active={tab} onChange={(t) => void switchTab(t)}
            tabs={[{ id: "form", label: "Form" }, { id: "xml", label: "XML" }]} />
          {tab === "form" && form && <ElementFormView spec={spec} form={form} onChange={setForm} />}
          {tab === "xml" && (
            <label className="party-field">
              Aurora XML (one element)
              <textarea className="input hb-xml" aria-label="Element XML" spellCheck={false} rows={18} value={xml} onChange={(e) => setXml(e.target.value)} />
            </label>
          )}
          <div className="party-line hb-actions">
            <Button disabled={busy} onClick={() => void check()}>Check</Button>
            <Button variant="primary" disabled={busy} onClick={() => void save()}>{busy ? "Working…" : "Save"}</Button>
            {elementId && <Button variant="danger" disabled={busy} onClick={() => void remove()}>Delete</Button>}
            {/* The user's own AI drafts and checks homebrew with the same engine (Account, Your AI). */}
            <AssistantButton purpose="homebrew" />
          </div>
          {report && <ReportView report={report} />}
        </Card>
      )}
    </div>
  );
}
